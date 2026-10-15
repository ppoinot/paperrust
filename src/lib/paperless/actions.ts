import { api, apiDelete, apiForm, apiPatch, apiPost, getDocument } from "./api";
import { entityFromPost } from "./format";
import {
  buildLlmKeyRequestBody,
  httpStatusOf,
  LLM_KEYS_PATH,
  llmKeyMutationErrorMessage,
  normalizeLlmKeyPatchResult,
  normalizeSettingsPayload,
  SETTINGS_PATH,
  type LlmKeyPatchResult,
  type SettingsPayload,
} from "./llm-key-status";
import {
  acceptPaperlessJobId,
  buildPaperlessImportBody,
  isSafePaperlessJobId,
  normalizePaperlessImportJob,
  validatePaperlessHttpsUrl,
  type PaperlessImportJob,
  type PaperlessImportStartBody,
  type PaperlessImportStartResponse,
} from "./paperless-import";
import type {
  Catalog,
  DashboardStats,
  DocumentDetail,
  DocumentFilter,
  DocumentListItem,
  Correspondent,
  DocumentType,
  LlmProvider,
  Tag,
} from "./types";
import type { StirlingCommitResult, StirlingEditSession } from "./stirling-edit";
export { isPdfDocument } from "./stirling-edit";
export type { StirlingCommitResult, StirlingEditSession, StirlingUploadStatus } from "./stirling-edit";
export type { LlmKeyStatus, SettingsPayload } from "./llm-key-status";

type Data<T> = { data: T };

export async function fetchCatalog(): Promise<Catalog> {
  return api<Catalog>("/api/catalog");
}

export async function fetchDashboard(): Promise<DashboardStats> {
  return api<DashboardStats>("/api/dashboard");
}

export async function fetchDocuments({ data }: Data<DocumentFilter>): Promise<DocumentListItem[]> {
  // inboxOnly is filtered server-side (is_inbox_tag OR name "inbox").
  return apiPost<DocumentListItem[]>("/api/documents/query", data);
}

export async function fetchDocument({ data }: Data<{ id: number }>): Promise<DocumentDetail | null> {
  return getDocument<DocumentDetail>(data.id);
}

export async function searchDocuments({ data }: Data<{ q: string }>): Promise<DocumentListItem[]> {
  if (!data.q.trim()) return [];
  return api<DocumentListItem[]>(`/api/search?q=${encodeURIComponent(data.q)}`);
}

export async function updateDocumentMeta({
  data,
}: Data<{
  id: number;
  title?: string;
  content?: string;
  correspondentId?: number | null;
  documentTypeId?: number | null;
  storagePathId?: number | null;
  createdDate?: string | null;
  asn?: number | null;
  tagIds?: number[];
  customValues?: { fieldId: number; value: string | null }[];
}>): Promise<DocumentDetail | null> {
  return apiPost<DocumentDetail | null>(`/api/documents/${data.id}/meta`, data);
}

export async function bulkUpdateDocuments({
  data,
}: Data<{
  ids: number[];
  addTagIds?: number[];
  removeTagIds?: number[];
  correspondentId?: number | null;
  documentTypeId?: number | null;
  deleteSelected?: boolean;
  restoreSelected?: boolean;
  purgeSelected?: boolean;
}>): Promise<{ ok: boolean; count: number }> {
  return apiPost("/api/documents/bulk", data);
}

export async function addNote({ data }: Data<{ documentId: number; body: string }>): Promise<DocumentDetail | null> {
  return apiPost("/api/notes", data);
}

export async function uploadDocument({
  data,
}: Data<{
  file: File | Blob;
  filename?: string;
  provider: LlmProvider;
  apiKey?: string;
  /** OpenAI-compatible base URL when provider is `selfhosted`. */
  baseUrl?: string;
}>): Promise<{ documentId: number; ocr: unknown; usedLlm: boolean; error?: string; ocrStatus?: string }> {
  const form = new FormData();
  const name =
    data.filename?.trim() || (data.file instanceof File && data.file.name ? data.file.name : "upload.bin");
  form.append("file", data.file, name);
  form.append("provider", data.provider);
  // do not send baseUrl on consume — OCR endpoint is settings-only.
  // Per-user key is loaded server-side; do not send keys from the browser in prod.
  return apiForm("/api/documents/consume", form);
}

export async function rerunOcr({
  data,
}: Data<{
  id: number;
  provider: LlmProvider;
  apiKey?: string;
}>): Promise<{ ok: boolean; error?: string }> {
  return apiPost(`/api/documents/${data.id}/ocr`, {
    id: data.id,
    provider: data.provider,
  });
}

export async function startPdfStirlingEdit(documentId: number): Promise<StirlingEditSession> {
  return apiPost<StirlingEditSession>(`/api/documents/${documentId}/edit/stirling`);
}

/** Cookie session (Paperrust tab). Multipart field `file` = edited PDF. */
export async function commitPdfStirlingEdit(
  documentId: number,
  file: File | Blob,
  token?: string,
  filename?: string,
): Promise<StirlingCommitResult> {
  const form = new FormData();
  const name =
    filename?.trim() || (file instanceof File && file.name ? file.name : "document.pdf");
  form.append("file", file, name);
  if (token?.trim()) form.append("token", token.trim());
  return apiForm<StirlingCommitResult>(`/api/documents/${documentId}/edit/stirling/commit`, form);
}

/** After a successful Stirling storage upload (`stirlingUpload === "ok"`). */
export async function pullPdfStirlingEdit(
  documentId: number,
  token?: string,
): Promise<StirlingCommitResult> {
  return apiPost<StirlingCommitResult>(`/api/documents/${documentId}/edit/stirling/commit`, {
    pull: true,
    token,
  });
}

export async function createNamedTag(name: string): Promise<Tag> {
  const raw = await apiPost<Tag | Tag[]>("/api/tags", { name });
  const tag = entityFromPost(raw, name);
  if (!tag) throw new Error("Étiquette créée introuvable");
  return tag;
}

export async function createNamedType(name: string): Promise<DocumentType> {
  const raw = await apiPost<DocumentType | DocumentType[]>("/api/types", { name });
  const created = entityFromPost(raw, name);
  if (!created) throw new Error("Type créé introuvable");
  return created;
}

/** Create (or return CI-existing) correspondent by name only. */
export async function createNamedCorrespondent(name: string): Promise<Correspondent> {
  const raw = await apiPost<Correspondent | Correspondent[]>("/api/correspondents", { name });
  const created = entityFromPost(raw, name);
  if (!created) throw new Error("Correspondant créé introuvable");
  return created;
}

export async function upsertTag({
  data,
}: Data<{
  id?: number;
  name: string;
  color: string;
  isInboxTag: boolean;
  match: string;
  matchingAlgorithm: string;
}>): Promise<Tag> {
  const raw = await apiPost<Tag | Tag[]>("/api/tags", data);
  const tag = entityFromPost(raw, data.name);
  if (!tag) throw new Error("Étiquette introuvable");
  return tag;
}

export async function deleteTag({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/tags/${data.id}`);
}

export async function upsertCorrespondent({
  data,
}: Data<{
  id?: number;
  name: string;
  match: string;
  matchingAlgorithm: string;
  notes?: string;
}>): Promise<Correspondent> {
  return apiPost("/api/correspondents", data);
}

export async function deleteCorrespondent({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/correspondents/${data.id}`);
}

export async function upsertType({
  data,
}: Data<{ id?: number; name: string; match: string; matchingAlgorithm: string }>): Promise<DocumentType> {
  const raw = await apiPost<DocumentType | DocumentType[]>("/api/types", data);
  const created = entityFromPost(raw, data.name);
  if (!created) throw new Error("Type introuvable");
  return created;
}

export async function deleteType({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/types/${data.id}`);
}

export async function upsertPath({
  data,
}: Data<{ id?: number; name: string; path: string; match: string; matchingAlgorithm: string }>): Promise<{
  ok: boolean;
}> {
  return apiPost("/api/storage-paths", data);
}

export async function deletePath({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/storage-paths/${data.id}`);
}

export async function upsertField({
  data,
}: Data<{ id?: number; name: string; dataType: string }>): Promise<{ ok: boolean }> {
  return apiPost("/api/fields", data);
}

export async function deleteField({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/fields/${data.id}`);
}

export async function upsertView({
  data,
}: Data<{
  id?: number;
  name: string;
  showOnDashboard: boolean;
  showInSidebar: boolean;
  sortField: string;
  sortReverse: boolean;
  filter: DocumentFilter;
}>): Promise<{ ok: boolean }> {
  return apiPost("/api/views", data);
}

export async function deleteView({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/views/${data.id}`);
}

export async function upsertWorkflow({
  data,
}: Data<{
  id?: number;
  name: string;
  enabled: boolean;
  triggerType: string;
  filters: Record<string, string | boolean>;
  actions: unknown[];
}>): Promise<{ ok: boolean }> {
  return apiPost("/api/workflows", data);
}

export async function deleteWorkflow({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/workflows/${data.id}`);
}

export async function simulateMailFetch({
  data,
}: Data<{ ruleId: number }>): Promise<{ ok: true; documentId: number } | { ok: false; error: string }> {
  return apiPost("/api/mail/simulate", data);
}

export async function fetchJobs(): Promise<
  {
    id: number;
    filename: string;
    status: string;
    message: string | null;
    document_id: number | null;
    created_at: string;
    finished_at: string | null;
  }[]
> {
  return api("/api/jobs");
}

export async function saveSettings({
  data,
}: Data<{
  ocrProvider: LlmProvider;
  ocrLanguage: string;
  autoMatch: boolean;
  /** OpenAI-compatible base URL when provider is `selfhosted`. */
  ocrBaseUrl?: string;
  trashRetentionDays?: number;
}>): Promise<{ ok: boolean }> {
  return apiPost(SETTINGS_PATH, data);
}

export async function fetchSettings(): Promise<SettingsPayload> {
  const raw = await api<unknown>(SETTINGS_PATH);
  return normalizeSettingsPayload(raw);
}

export async function patchLlmApiKey({
  data,
}: Data<{ provider: LlmProvider | string; apiKey?: string; clear?: boolean }>): Promise<LlmKeyPatchResult> {
  const body = buildLlmKeyRequestBody(data);
  try {
    let raw: unknown;
    try {
      // PATCH /api/settings/llm-keys + provider=xai is the live contract.
      // Never call /api/settings/llm (404 on beta).
      raw = await apiPatch<unknown>(LLM_KEYS_PATH, body);
    } catch (err) {
      const status = httpStatusOf(err);
      if (status === 404 || status === 405) {
        raw = await apiPost<unknown>(LLM_KEYS_PATH, body);
      } else {
        throw err;
      }
    }
    return normalizeLlmKeyPatchResult(raw, body.provider);
  } catch (err) {
    const wrapped = new Error(llmKeyMutationErrorMessage(err, body.provider)) as Error & {
      status?: number;
    };
    wrapped.status = httpStatusOf(err);
    throw wrapped;
  }
}

export type ApiTokenRow = {
  id: number;
  name: string;
  prefix: string;
  createdAt: string;
  lastUsedAt: string | null;
  revokedAt: string | null;
};

export async function fetchApiTokens(): Promise<ApiTokenRow[]> {
  const res = await api<{ tokens: ApiTokenRow[] }>("/api/settings/api-tokens");
  return res.tokens ?? [];
}

export async function createApiToken({
  data,
}: Data<{ name?: string }>): Promise<{
  ok: boolean;
  id: number;
  name: string;
  prefix: string;
  token: string;
  createdAt: string;
}> {
  return apiPost("/api/settings/api-tokens", { name: data.name ?? "" });
}

export async function revokeApiToken({ data }: Data<{ id: number }>): Promise<{ ok: boolean }> {
  return apiDelete(`/api/settings/api-tokens/${data.id}`);
}

function withHttpStatus(message: string, status: number): Error & { status: number } {
  const err = new Error(message) as Error & { status: number };
  err.status = status;
  return err;
}

function remapPaperlessImportHttpError(err: unknown): never {
  const status = err instanceof Error ? (err as Error & { status?: number }).status : undefined;
  if (status === 409) {
    throw withHttpStatus("Un import Paperless-ngx est déjà en cours pour ce compte.", 409);
  }
  if (status === 404) {
    throw withHttpStatus("L'import Paperless-ngx n'est pas encore disponible sur le serveur.", 404);
  }
  throw err;
}

/** POST /api/settings/paperless-import — 202 `{ jobId }`. Token is request-only. */
export async function startPaperlessImport(
  input: PaperlessImportStartBody,
): Promise<PaperlessImportStartResponse> {
  const body = buildPaperlessImportBody(input);
  const httpsErr = validatePaperlessHttpsUrl(body.baseUrl);
  if (httpsErr) throw new Error(httpsErr);
  try {
    const res = await apiPost<PaperlessImportStartResponse>("/api/settings/paperless-import", body);
    const jobId = acceptPaperlessJobId(res?.jobId) ?? "";
    return { jobId };
  } catch (err) {
    remapPaperlessImportHttpError(err);
  }
}

export async function fetchPaperlessImportJob(jobId: string): Promise<PaperlessImportJob> {
  if (!isSafePaperlessJobId(jobId)) {
    throw new Error("Identifiant d'import invalide.");
  }
  try {
    const raw = await api<unknown>(`/api/settings/paperless-import/${encodeURIComponent(jobId)}`);
    return normalizePaperlessImportJob(raw, jobId);
  } catch (err) {
    remapPaperlessImportHttpError(err);
  }
}

export async function cancelPaperlessImport(jobId: string): Promise<{ ok: true }> {
  if (!isSafePaperlessJobId(jobId)) {
    throw new Error("Identifiant d'import invalide.");
  }
  try {
    return await apiPost<{ ok: true }>(
      `/api/settings/paperless-import/${encodeURIComponent(jobId)}/cancel`,
    );
  } catch (err) {
    remapPaperlessImportHttpError(err);
  }
}
