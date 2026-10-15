//! Upload size, memory, and PDFium concurrency guards.

pub const MAX_TITLE_CHARS: usize = 500;
pub const MAX_FILENAME_CHARS: usize = 255;

/// Clamp document titles to [`MAX_TITLE_CHARS`] (Unicode scalar boundaries).
pub fn clamp_title(s: &str) -> String {
    s.chars().take(MAX_TITLE_CHARS).collect()
}

/// Clamp upload / original filenames to [`MAX_FILENAME_CHARS`].
pub fn clamp_filename(s: &str) -> String {
    s.chars().take(MAX_FILENAME_CHARS).collect()
}

use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default JSON / settings body cap (). Large uploads use [`MAX_UPLOAD_BYTES`]
/// only on consume / ingest / post_document routes.
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Practical upload / OCR cap (). Was 300 MiB; lowered to limit memory amplification
/// (raw + rendered JPEGs + INSERT). Keep Ingress `proxy-body-size` ≥ this value.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Concurrent PDFium / heavy render+OCR jobs (/ ).
const HEAVY_JOB_PERMITS: usize = 2;

/// Max wait for a heavy-job permit (). Avoids queueing forever behind stuck PDFium.
pub const HEAVY_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

static HEAVY_JOBS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(HEAVY_JOB_PERMITS)));

/// Acquire a permit for PDFium `spawn_blocking` / heavy OCR prep (bounded wait).
pub async fn acquire_heavy_job() -> Result<OwnedSemaphorePermit, anyhow::Error> {
    acquire_heavy_job_within(HEAVY_ACQUIRE_TIMEOUT).await
}

/// Acquire with an explicit wait budget (used by tests with a short timeout).
pub async fn acquire_heavy_job_within(
    wait: Duration,
) -> Result<OwnedSemaphorePermit, anyhow::Error> {
    match tokio::time::timeout(wait, HEAVY_JOBS.clone().acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) => Err(anyhow::anyhow!("file d'attente rendu/OCR fermée — réessayez")),
        Err(_) => Err(anyhow::anyhow!(
            "file d'attente rendu/OCR saturée — réessayez"
        )),
    }
}

/// Run a blocking heavy job under the global semaphore + timeout.
///
/// On timeout the caller gets an error **immediately**. The JoinHandle is detached
/// into a background task that keeps the semaphore permit until PDFium finishes
/// (`spawn_blocking` cannot cancel in-process work). Do not `join.await` on the
/// request path.
pub async fn run_heavy_blocking<T, F>(
    timeout: Duration,
    f: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    let permit = acquire_heavy_job().await?;
    let mut join = tokio::task::spawn_blocking(f);
    tokio::select! {
        res = &mut join => {
            drop(permit);
            match res {
                Ok(inner) => inner,
                Err(join_err) => Err(anyhow::anyhow!("tâche rendu interrompue: {join_err}")),
            }
        }
        _ = tokio::time::sleep(timeout) => {
            // Detach: hold permit off the hot path until the blocking thread ends.
            tokio::spawn(async move {
                let _permit = permit;
                let _ = join.await;
            });
            Err(anyhow::anyhow!(
                "délai de rendu dépassé ({}s)",
                timeout.as_secs()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::LazyLock as TestLazyLock;
    use tokio::sync::Mutex as AsyncMutex;

    /// Global semaphore is process-wide — serialize tests that take permits.
    static SEM_TEST_LOCK: TestLazyLock<AsyncMutex<()>> = TestLazyLock::new(|| AsyncMutex::new(()));

    #[test]
    fn default_body_is_small() {
        assert!(DEFAULT_BODY_LIMIT_BYTES <= 2 * 1024 * 1024);
        assert!(DEFAULT_BODY_LIMIT_BYTES < MAX_UPLOAD_BYTES);
    }

    #[test]
    fn upload_cap_is_practical() {
        // well below the historical 300 MiB API-wide limit.
        assert!(MAX_UPLOAD_BYTES <= 64 * 1024 * 1024);
        assert!(MAX_UPLOAD_BYTES >= 16 * 1024 * 1024);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn heavy_semaphore_limits_concurrency() {
        let _guard = SEM_TEST_LOCK.lock().await;
        let p1 = acquire_heavy_job().await.expect("p1");
        let p2 = acquire_heavy_job().await.expect("p2");
        let wait = acquire_heavy_job_within(Duration::from_millis(50)).await;
        assert!(wait.is_err(), "third permit must time out while two are held");
        drop(p1);
        let p3 = acquire_heavy_job_within(Duration::from_secs(1))
            .await
            .expect("p3");
        drop(p2);
        drop(p3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn heavy_timeout_returns_without_awaiting_join() {
        use std::sync::Arc;
        use std::time::Instant;

        let _guard = SEM_TEST_LOCK.lock().await;
        let started = Arc::new(tokio::sync::Notify::new());
        let started2 = started.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = finished.clone();

        let timeout = Duration::from_millis(50);
        let block_for = Duration::from_millis(400);

        let t0 = Instant::now();
        let result = run_heavy_blocking(timeout, move || {
            started2.notify_one();
            std::thread::sleep(block_for);
            finished2.store(true, Ordering::SeqCst);
            Ok::<_, anyhow::Error>(())
        })
        .await;
        let elapsed = t0.elapsed();

        let err = result.expect_err("must time out");
        assert!(err.to_string().contains("délai"), "{err}");
        // Hot path must return near the timeout — not after the full blocking sleep.
        assert!(
            elapsed < Duration::from_millis(200),
            "timed-out call must return promptly, elapsed={elapsed:?}"
        );
        assert!(
            !finished.load(Ordering::SeqCst),
            "blocking work must still be running when the caller returns"
        );

        // Permit is held in the background until PDFium/thread ends — second free
        // slot exists, but a third waiter still times out while the detached job runs.
        started.notified().await;
        let p_extra = acquire_heavy_job_within(Duration::from_millis(100))
            .await
            .expect("second permit");
        let third = acquire_heavy_job_within(Duration::from_millis(50)).await;
        assert!(
            third.is_err(),
            "third must wait while detached timed-out job still holds a permit"
        );
        drop(p_extra);

        // Let the detached join finish so later tests see a free semaphore.
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert!(finished.load(Ordering::SeqCst));
    }
}
