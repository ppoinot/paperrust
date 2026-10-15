import {
  Activity,
  FileStack,
  Files,
  FolderTree,
  Inbox,
  LayoutDashboard,
  Mail,
  Settings,
  Shapes,
  Tags,
  Trash2,
  Users,
  Waypoints,
  Workflow,
} from "lucide-react";

export const NAV = [
  { to: "/", label: "Tableau de bord", icon: LayoutDashboard, exact: true },
  { to: "/inbox", label: "Boîte de réception", icon: Inbox, badge: "inbox" as const },
  { to: "/documents", label: "Documents", icon: Files },
  { to: "/trash", label: "Corbeille", icon: Trash2 },
];

export const MANAGE = [
  { to: "/correspondents", label: "Correspondants", icon: Users },
  { to: "/tags", label: "Étiquettes", icon: Tags },
  { to: "/types", label: "Types", icon: Shapes },
  { to: "/storage-paths", label: "Chemins", icon: FolderTree },
  { to: "/fields", label: "Champs", icon: FileStack },
];

export const AUTOMATION = [
  { to: "/views", label: "Vues enregistrées", icon: Waypoints },
  { to: "/workflows", label: "Workflows", icon: Workflow },
  { to: "/mail", label: "Courrier", icon: Mail },
  { to: "/logs", label: "Journal", icon: Activity },
  { to: "/settings", label: "Réglages", icon: Settings },
];
