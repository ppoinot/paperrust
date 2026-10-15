import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { AppShell } from "@/components/layout/app-shell";
import { PaperlessImportSection } from "@/components/settings/paperless-import-section";
import { SettingsToc, type SettingsTocItem } from "@/components/settings/settings-toc";
import { PAPERLESS_IMPORT_TOC_ID } from "@/lib/paperless/paperless-import";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { fetchApiTokens, createApiToken, revokeApiToken, fetchSettings, patchLlmApiKey, saveSettings } from "@/lib/paperless/actions";
import { errorMessageFromUnknown } from "@/lib/http-error";
import {
  llmKeyConfiguredBadge,
  llmKeyConfiguredToast,
  llmKeyStatusForProvider,
  mergeLlmKeyIntoSettings,
  resolveHasConfiguredKey,
  writeConfiguredProvider,
  type SettingsPayload,
} from "@/lib/paperless/llm-key-status";
import {
  API_KEY_PROVIDERS,
  draftKeyForProvider,
  keyLabel,
  keyPlaceholder,
  providerSelectLabel,
  readLlmSettings,
  withDraftKey,
  writeLlmSettings,
  type ClientLlmSettings,
} from "@/lib/paperless/llm-settings";
import type { LlmProvider } from "@/lib/paperless/types";
import {
  LLM_OAUTH_PROVIDERS,
  OAUTH_SOON_FR,
  disconnectLlmOAuth,
  isLlmOAuthAvailable,
  readLlmOAuthConnection,
  startLlmOAuthLogin,
  type OAuthLlmProvider,
} from "@/lib/paperless/llm-oauth";
import {
  STIRLING_SETTINGS_HELP,
  STIRLING_SETTINGS_TOC_ID,
  loadStirlingBaseUrl,
  readStirlingBaseUrlOverride,
  writeStirlingBaseUrlOverride,
} from "@/lib/paperless/stirling-edit";

export const Route = createFileRoute("/settings")({ component: Page });

const SELECTABLE: LlmProvider[] = ["xai", "openai", "anthropic", "selfhosted"];

const SETTINGS_TOC: readonly SettingsTocItem[] = [
  { id: "settings-ocr", label: "OCR par LLM" },
  { id: "settings-api-tokens", label: "Tokens API" },
  { id: PAPERLESS_IMPORT_TOC_ID, label: "Import Paperless-ngx" },
  { id: STIRLING_SETTINGS_TOC_ID, label: "Édition PDF" },
  { id: "settings-archive", label: "Archive" },
  { id: "settings-corbeille", label: "Corbeille" },
];

function Page() {
  const qc = useQueryClient();
  const remote = useQuery({ queryKey: ["settings"], queryFn: () => fetchSettings() });
  const tokensQ = useQuery({ queryKey: ["api-tokens"], queryFn: () => fetchApiTokens() });
  const [llm, setLlm] = useState<ClientLlmSettings>(readLlmSettings);
  const [language, setLanguage] = useState("fr");
  const [autoMatch, setAutoMatch] = useState(true);
  const [trashRetentionDays, setTrashRetentionDays] = useState<30 | 60 | 90>(30);
  const [stirlingBaseUrl, setStirlingBaseUrl] = useState(readStirlingBaseUrlOverride);
  const [stirlingPlaceholder, setStirlingPlaceholder] = useState("https://pdf.example");
  const [tokenName, setTokenName] = useState("iPhone");
  const [freshToken, setFreshToken] = useState<string | null>(null);
  const [oauthByProvider] = useState(
    () =>
      Object.fromEntries(
        LLM_OAUTH_PROVIDERS.map((p) => [p.id, readLlmOAuthConnection(p.id)]),
      ) as Record<OAuthLlmProvider, ReturnType<typeof readLlmOAuthConnection>>,
  );

  useEffect(() => {
    let cancelled = false;
    void loadStirlingBaseUrl().then((url) => {
      if (!cancelled && url) setStirlingPlaceholder(url);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!remote.data) return;
    setLanguage(remote.data.ocrLanguage);
    setAutoMatch(remote.data.autoMatch);
    const d = remote.data.trashRetentionDays;
    setTrashRetentionDays(d === 60 || d === 90 ? d : 30);
    const remoteProvider = remote.data.ocrProvider as LlmProvider;
    const remoteBase = remote.data.ocrBaseUrl?.trim() || undefined;
    setLlm((prev) => {
      const next = {
        ...prev,
        provider: SELECTABLE.includes(remoteProvider) ? remoteProvider : prev.provider,
        selfhostedBaseUrl: remoteBase || prev.selfhostedBaseUrl,
      };
      writeLlmSettings(next);
      return next;
    });
  }, [remote.data]);

  const keyStatus = llmKeyStatusForProvider(remote.data?.llmKeys, llm.provider);
  const draft = draftKeyForProvider(llm, llm.provider);
  const supportsKey = (API_KEY_PROVIDERS as readonly string[]).includes(llm.provider);
  const hasKey = resolveHasConfiguredKey(
    llm.provider,
    remote.data?.llmKeys,
    Boolean(remote.data?.llmKeysFromApi),
  );
  const isSelfhosted = llm.provider === "selfhosted";

  function setProvider(next: LlmProvider) {
    const updated = { ...llm, provider: next };
    setLlm(updated);
    writeLlmSettings(updated);
  }

  const save = useMutation({
    mutationFn: async () => {
      if (isSelfhosted) {
        const base = llm.selfhostedBaseUrl.trim();
        if (!base) {
          throw new Error("URL de base requise pour Auto-hébergé (ex. https://llm.exemple.org/v1).");
        }
        if (!/^https:\/\//i.test(base)) {
          throw new Error("L'URL OCR Auto-hébergé doit commencer par https:// (HTTP en clair refusé).");
        }
        try {
          const u = new URL(base);
          const host = u.hostname.toLowerCase();
          if (
            host === "localhost" ||
            host.endsWith(".localhost") ||
            host === "127.0.0.1" ||
            host === "::1" ||
            host === "169.254.169.254" ||
            /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[0-1])\.)/.test(host)
          ) {
            throw new Error(
              "URL OCR refusée : hôte privé, loopback ou métadonnées cloud interdit.",
            );
          }
        } catch (e) {
          if (e instanceof Error && e.message.startsWith("URL OCR")) throw e;
          if (e instanceof Error && e.message.startsWith("L'URL OCR")) throw e;
          throw new Error("URL de base OCR invalide — utilisez une URL https:// complète.");
        }
      }
      writeLlmSettings(llm);
      let keySaved = false;
      if (supportsKey && draft.trim()) {
        const status = await patchLlmApiKey({ data: { provider: llm.provider, apiKey: draft.trim() } });
        writeConfiguredProvider(status.provider || llm.provider, status.hasKey);
        qc.setQueryData(["settings"], (prev: SettingsPayload | undefined) =>
          mergeLlmKeyIntoSettings(prev, status),
        );
        setLlm((prev) => withDraftKey(prev, llm.provider, ""));
        keySaved = status.hasKey;
      }
      await saveSettings({
        data: {
          ocrProvider: llm.provider,
          ocrLanguage: language,
          autoMatch,
          ocrBaseUrl: llm.selfhostedBaseUrl.trim(),
          trashRetentionDays,
        },
      });
      return { keySaved };
    },
    onSuccess: (res) => {
      qc.invalidateQueries({ queryKey: ["settings"] });
      toast.success(
        res.keySaved
          ? `Réglages enregistrés. ${llmKeyConfiguredToast(llm.provider)}`
          : "Réglages enregistrés",
      );
    },
    onError: (e: Error) => toast.error(errorMessageFromUnknown(e)),
  });

  const clearKey = useMutation({
    mutationFn: async () => {
      const status = await patchLlmApiKey({ data: { provider: llm.provider, clear: true } });
      writeConfiguredProvider(status.provider || llm.provider, false);
      qc.setQueryData(["settings"], (prev: SettingsPayload | undefined) =>
        mergeLlmKeyIntoSettings(prev, { ...status, hasKey: false }),
      );
      setLlm((prev) => withDraftKey(prev, llm.provider, ""));
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["settings"] });
      toast.success("Clé API supprimée");
    },
    onError: (e: Error) => toast.error(errorMessageFromUnknown(e)),
  });

  const createToken = useMutation({
    mutationFn: async () => createApiToken({ data: { name: tokenName.trim() || "iPhone" } }),
    onSuccess: (res) => {
      setFreshToken(res.token);
      qc.invalidateQueries({ queryKey: ["api-tokens"] });
      toast.success("Token créé — copiez-le maintenant, il ne sera plus affiché.");
    },
    onError: (e: Error) => toast.error(e.message),
  });

  const revokeToken = useMutation({
    mutationFn: async (id: number) => revokeApiToken({ data: { id } }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["api-tokens"] });
      toast.success("Token révoqué");
    },
    onError: (e: Error) => toast.error(e.message),
  });

  return (
    <AppShell title="Réglages">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 lg:flex-row lg:items-start lg:gap-10">
        <SettingsToc items={SETTINGS_TOC} />
        <div className="min-w-0 w-full max-w-xl flex-1 space-y-8 lg:max-w-xl">
        {remote.isError ? (
          <p className="rounded-lg bg-surface px-3 py-2 text-sm text-danger shadow-[var(--shadow-border)]">
            Impossible de charger les réglages : {(remote.error as Error).message}
          </p>
        ) : null}
        <section id="settings-ocr" className="scroll-mt-24 rounded-xl bg-surface p-5 shadow-[var(--shadow-border)]">
          <h2 className="font-display text-xl">OCR par LLM</h2>
          <p className="mt-1 text-sm text-muted">
            À l&apos;import, Paperrust envoie l&apos;image au modèle choisi pour extraire le texte, le
            titre, le correspondant et les étiquettes. Chaque compte utilise sa propre clé API.
          </p>
          <div className="mt-4 space-y-3">
            <div>
              <Label>Fournisseur</Label>
              <Select value={llm.provider} onValueChange={(v) => setProvider(v as LlmProvider)}>
                <SelectTrigger className="mt-1">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SELECTABLE.map((id) => (
                    <SelectItem key={id} value={id}>
                      {providerSelectLabel(id)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="mt-1 text-xs text-faint">
                {hasKey
                  ? "Clé configurée sur le serveur pour ce fournisseur."
                  : isSelfhosted
                    ? "URL de base requise ; la clé API est optionnelle selon votre serveur."
                    : "Ajoutez votre clé API ci-dessous pour activer l'OCR avec ce fournisseur."}
              </p>
            </div>

            {isSelfhosted ? (
              <div>
                <Label>URL de base</Label>
                <Input
                  className="mt-1 font-mono"
                  type="url"
                  autoComplete="off"
                  placeholder="https://llm.exemple.loc/v1"
                  value={llm.selfhostedBaseUrl}
                  onChange={(e) => {
                    const updated = { ...llm, selfhostedBaseUrl: e.target.value };
                    setLlm(updated);
                    writeLlmSettings(updated);
                  }}
                />
                <p className="mt-1 text-xs text-faint">
                  Endpoint compatible OpenAI (…/v1). Mémorisée avec le fournisseur actif pour
                  l&apos;import.
                </p>
              </div>
            ) : null}

            <div className="space-y-3 rounded-lg bg-surface-2/80 px-3 py-3">
              <div>
                <p className="text-sm font-medium text-fg">Connexion OAuth (compte)</p>
                <p className="mt-0.5 text-xs text-muted">
                  {OAUTH_SOON_FR} Fournisseurs : SpaceXAi, OpenAI, Anthropic, Kimi, Mistral — ou
                  Auto-hébergé.
                </p>
              </div>
              <ul className="space-y-2">
                {LLM_OAUTH_PROVIDERS.map((provider) => {
                  const available = isLlmOAuthAvailable(provider.id);
                  const conn = oauthByProvider[provider.id];
                  return (
                    <li
                      key={provider.id}
                      className="flex flex-wrap items-center justify-between gap-2 rounded-md bg-surface/60 px-2.5 py-2"
                    >
                      <div className="min-w-0">
                        <p className="text-sm text-fg">{provider.title}</p>
                        <p className="text-xs text-faint">
                          {available
                            ? conn.connected
                              ? `Connecté${conn.label ? ` — ${conn.label}` : ""}`
                              : "Utiliser votre abonnement pour l'OCR."
                            : OAUTH_SOON_FR}
                        </p>
                      </div>
                      {available && conn.connected ? (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => disconnectLlmOAuth(provider.id)}
                        >
                          Déconnecter
                        </Button>
                      ) : (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={!available}
                          title={available ? undefined : OAUTH_SOON_FR}
                          onClick={() => startLlmOAuthLogin(provider.id)}
                        >
                          Se connecter avec {provider.buttonLabel}
                        </Button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>

            {supportsKey ? (
              <div>
                <div className="flex flex-wrap items-center gap-2">
                  <Label>{keyLabel(llm.provider)}</Label>
                  {hasKey ? (
                    <span
                      role="status"
                      className="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[11px] font-medium text-emerald-700 dark:text-emerald-300"
                    >
                      {llmKeyConfiguredBadge()}
                    </span>
                  ) : null}
                </div>
                {hasKey ? (
                  <p className="mt-1 font-mono text-sm text-muted" aria-hidden={true}>
                    {keyStatus?.masked?.trim() ? keyStatus.masked : "••••••••"}
                  </p>
                ) : null}
                <Input
                  className="mt-1 font-mono"
                  type="password"
                  autoComplete="off"
                  value={draft}
                  onChange={(e) => setLlm(withDraftKey(llm, llm.provider, e.target.value))}
                  placeholder={keyPlaceholder(llm.provider, hasKey)}
                />
                <div className="mt-2 flex flex-wrap items-center gap-2">
                  <p className="flex-1 text-xs text-faint">
                    {hasKey
                      ? "Champ vide normal : le secret n'est jamais réaffiché. Saisissez une nouvelle clé pour la remplacer."
                      : isSelfhosted
                        ? "Optionnelle si votre serveur n'exige pas d'authentification."
                        : "Stockée chiffrée sur le serveur, liée à votre compte. Jamais renvoyée en clair."}
                  </p>
                  {hasKey ? (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={clearKey.isPending}
                      onClick={() => clearKey.mutate()}
                    >
                      Effacer la clé
                    </Button>
                  ) : null}
                </div>
              </div>
            ) : null}

            <div>
              <Label>Langue OCR par défaut</Label>
              <Select value={language} onValueChange={setLanguage}>
                <SelectTrigger className="mt-1">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="fr">Français</SelectItem>
                  <SelectItem value="en">English</SelectItem>
                  <SelectItem value="de">Deutsch</SelectItem>
                  <SelectItem value="it">Italiano</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <label className="flex items-center justify-between gap-3 text-sm">
              <span>Classement automatique (motifs + LLM)</span>
              <Switch checked={autoMatch} onCheckedChange={setAutoMatch} />
            </label>
          </div>
        </section>
        <section id="settings-api-tokens" className="scroll-mt-24 rounded-xl bg-surface p-5 shadow-[var(--shadow-border)]">
          <h2 className="font-display text-xl">Tokens API (iPhone / Quickscan)</h2>
          <p className="mt-1 text-sm text-muted">
            Scope : ingest uniquement. Créez un token <span className="font-mono text-xs">pr_…</span> puis
            utilisez-le ainsi :
          </p>
          <ul className="mt-2 list-inside list-disc space-y-1 text-sm text-muted">
            <li>
              Raccourci iOS — <span className="font-mono text-xs">POST /api/v1/ingest</span> +{" "}
              <span className="font-mono text-xs">Authorization: Bearer pr_…</span>
            </li>
            <li>
              Quickscan (mode Paperless-ngx) — URL de base{" "}
              <span className="font-mono text-xs">{typeof window !== "undefined" ? window.location.origin : "l'adresse de cette archive"}</span>
              , token collé tel quel ; upload via{" "}
              <span className="font-mono text-xs">POST /api/documents/post_document/</span> (
              <span className="font-mono text-xs">Authorization: Token pr_…</span>).
            </li>
          </ul>
          <div className="mt-4 flex flex-wrap items-end gap-2">
            <div className="min-w-[10rem] flex-1">
              <Label>Nom</Label>
              <Input
                className="mt-1"
                value={tokenName}
                onChange={(e) => setTokenName(e.target.value)}
                placeholder="iPhone"
                maxLength={80}
              />
            </div>
            <Button
              type="button"
              disabled={createToken.isPending}
              onClick={() => createToken.mutate()}
            >
              Créer un token
            </Button>
          </div>
          {freshToken ? (
            <div className="mt-3 rounded-lg bg-amber-500/10 px-3 py-2 text-sm">
              <p className="font-medium text-fg">Secret (affichage unique)</p>
              <p className="mt-1 break-all font-mono text-xs text-fg">{freshToken}</p>
              <div className="mt-2 flex flex-wrap gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(freshToken);
                      toast.success("Token copié");
                    } catch {
                      toast.error("Copie impossible — sélectionnez le token manuellement.");
                    }
                  }}
                >
                  Copier
                </Button>
                <Button type="button" size="sm" variant="outline" onClick={() => setFreshToken(null)}>
                  Masquer
                </Button>
              </div>
            </div>
          ) : null}
          <ul className="mt-4 space-y-2">
            {(tokensQ.data ?? [])
              .filter((t) => !t.revokedAt)
              .map((t) => (
                <li
                  key={t.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-md bg-surface-2/80 px-2.5 py-2 text-sm"
                >
                  <div className="min-w-0">
                    <p className="text-fg">{t.name || "Token"}</p>
                    <p className="font-mono text-xs text-faint">
                      {t.prefix}… · créé {t.createdAt}
                      {t.lastUsedAt ? ` · utiliséé ${t.lastUsedAt}` : ""}
                    </p>
                  </div>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={revokeToken.isPending}
                    onClick={() => revokeToken.mutate(t.id)}
                  >
                    Révoquer
                  </Button>
                </li>
              ))}
            {tokensQ.isSuccess && (tokensQ.data ?? []).filter((t) => !t.revokedAt).length === 0 ? (
              <li className="text-xs text-faint">Aucun token actif.</li>
            ) : null}
          </ul>
        </section>
        <PaperlessImportSection />
        <section
          id={STIRLING_SETTINGS_TOC_ID}
          className="scroll-mt-24 rounded-xl bg-surface p-5 shadow-[var(--shadow-border)]"
        >
          <h2 className="font-display text-xl">Édition PDF (Stirling)</h2>
          <p className="mt-1 text-sm text-muted">{STIRLING_SETTINGS_HELP}</p>
          <div className="mt-4">
            <Label htmlFor="stirling-base-url">URL Stirling</Label>
            <Input
              id="stirling-base-url"
              className="mt-1 font-mono"
              type="url"
              autoComplete="off"
              placeholder={stirlingPlaceholder}
              value={stirlingBaseUrl}
              onChange={(e) => {
                const next = e.target.value;
                setStirlingBaseUrl(next);
                writeStirlingBaseUrlOverride(next);
              }}
            />
          </div>
        </section>
        <section id="settings-archive" className="scroll-mt-24 rounded-xl bg-surface p-5 shadow-[var(--shadow-border)]">
          <h2 className="font-display text-xl">Archive</h2>
          <ul className="mt-3 space-y-2 text-sm text-muted">
            <li>ASN séquentiel à partir de 1000</li>
            <li>Postgres (compatible protocole pgrust / libpq)</li>
            <li>Consommation : matching any / all / exact / regex / fuzzy / auto + workflows</li>
          </ul>
        </section>
        
          <section id="settings-corbeille" className="scroll-mt-24 space-y-2 rounded-xl bg-surface p-4 shadow-[var(--shadow-border)]">
            <h2 className="font-display text-lg">Corbeille</h2>
            <p className="text-sm text-muted">
              Les documents soft-supprimés sont purgés automatiquement après ce délai (fichiers d’origine inclus).
            </p>
            <div>
              <Label htmlFor="trash-retention">Conservation avant purge</Label>
              <Select
                value={String(trashRetentionDays)}
                onValueChange={(v) => setTrashRetentionDays(Number(v) as 30 | 60 | 90)}
              >
                <SelectTrigger id="trash-retention" className="mt-1 w-56">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="30">30 jours</SelectItem>
                  <SelectItem value="60">60 jours</SelectItem>
                  <SelectItem value="90">90 jours</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </section>

          <Button onClick={() => save.mutate()} disabled={save.isPending}>
            Enregistrer les réglages
          </Button>
        </div>
      </div>
    </AppShell>
  );
}
