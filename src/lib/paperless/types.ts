export type MatchingAlgorithm =
  | "none"
  | "any"
  | "all"
  | "exact"
  | "regex"
  | "fuzzy"
  | "auto";

export type OcrStatus = "pending" | "processing" | "done" | "failed" | "skipped";

export type LlmProvider = "xai" | "openai" | "anthropic" | "selfhosted";

export type Tag = {
  id: number;
  name: string;
  slug?: string;
  color: string;
  isInboxTag: boolean;
  match: string;
  matchingAlgorithm: MatchingAlgorithm;
  isInsensitive: boolean;
};

export type Correspondent = {
  id: number;
  name: string;
  match: string;
  matchingAlgorithm: MatchingAlgorithm;
  isInsensitive: boolean;
  notes: string | null;
};

export type DocumentType = {
  id: number;
  name: string;
  match: string;
  matchingAlgorithm: MatchingAlgorithm;
  isInsensitive: boolean;
};

export type StoragePath = {
  id: number;
  name: string;
  path: string;
  match: string;
  matchingAlgorithm: MatchingAlgorithm;
  isInsensitive: boolean;
};

export type CustomField = {
  id: number;
  name: string;
  dataType: string;
  extraData: Record<string, string>;
};

export type DocumentNote = {
  id: number;
  body: string;
  createdAt: string;
};

export type DocumentListItem = {
  id: number;
  title: string;
  content: string;
  correspondentId: number | null;
  correspondentName: string | null;
  documentTypeId: number | null;
  documentTypeName: string | null;
  storagePathId: number | null;
  storagePathName: string | null;
  createdDate: string | null;
  addedAt: string;
  modifiedAt: string;
  asn: number | null;
  mimeType: string;
  originalFilename: string | null;
  pageCount: number;
  language: string;
  ocrStatus: OcrStatus;
  ocrProvider: string | null;
  thumbnailSvg: string | null;
  fileSize: number;
  summary: string | null;
  tags: Tag[];
};

export type DocumentDetail = DocumentListItem & {
  fileData: string | null;
  /** Downloadable original at GET /api/documents/:id/file */
  hasOriginal?: boolean;
  notes: DocumentNote[];
  customValues: { fieldId: number; name: string; dataType: string; value: string | null }[];
  history: { id: number; action: string; detail: string | null; createdAt: string }[];
};

export type DocumentFilter = {
  q?: string;
  tagIds?: number[];
  correspondentId?: number | null;
  documentTypeId?: number | null;
  storagePathId?: number | null;
  inboxOnly?: boolean;
  trashOnly?: boolean;
  ocrStatus?: OcrStatus;
  dateFrom?: string;
  dateTo?: string;
  viewId?: number;
  sort?: string;
  sortReverse?: boolean;
};

export type SavedView = {
  id: number;
  name: string;
  showOnDashboard: boolean;
  showInSidebar: boolean;
  sortField: string;
  sortReverse: boolean;
  filter: DocumentFilter;
};

export type WorkflowFilters = {
  contentContains?: string;
  hasCorrespondent?: boolean;
};

export type Workflow = {
  id: number;
  name: string;
  enabled: boolean;
  triggerType: string;
  filters: WorkflowFilters;
  actions: WorkflowAction[];
};

export type WorkflowAction = {
  type: "add_tag" | "remove_tag" | "set_correspondent" | "set_type" | "set_storage";
  tagId?: number;
  correspondentId?: number;
  documentTypeId?: number;
  storagePathId?: number;
};

export type MailAccount = {
  id: number;
  name: string;
  imapServer: string | null;
  username: string | null;
  imapPort: number | null;
};

export type MailRule = {
  id: number;
  name: string;
  accountId: number | null;
  folder: string | null;
  filterFrom: string | null;
  filterSubject: string | null;
  enabled: boolean;
  correspondentId: number | null;
  documentTypeId: number | null;
};

export type ConsumeJob = {
  id: number;
  filename: string;
  status: string;
  message: string | null;
  documentId: number | null;
  createdAt: string;
  finishedAt: string | null;
};

export type DashboardStats = {
  documents: number;
  inbox: number;
  correspondents: number;
  tags: number;
  pages: number;
  storageBytes: number;
  ocrDone: number;
  ocrPending: number;
  recent: DocumentListItem[];
  byMonth: { month: string; count: number }[];
  byType: { name: string; count: number }[];
  jobs: ConsumeJob[];
  views: SavedView[];
};

export type Catalog = {
  tags: Tag[];
  correspondents: Correspondent[];
  documentTypes: DocumentType[];
  storagePaths: StoragePath[];
  customFields: CustomField[];
  savedViews: SavedView[];
  workflows: Workflow[];
  mailAccounts: MailAccount[];
  mailRules: MailRule[];
  aiAvailable: boolean;
};

export const MATCHING_LABELS: Record<MatchingAlgorithm, string> = {
  none: "Aucun",
  any: "N’importe lequel",
  all: "Tous les mots",
  exact: "Correspondance exacte",
  regex: "Expression régulière",
  fuzzy: "Flou",
  auto: "Automatique",
};

export const TYPE_ICONS: Record<string, string> = {
  Facture: "Receipt",
  Quittance: "Stamp",
  Contrat: "FileSignature",
  Relevé: "Landmark",
  Courrier: "Mail",
  Attestation: "BadgeCheck",
  Ordonnance: "HeartPulse",
  Garantie: "Shield",
};
