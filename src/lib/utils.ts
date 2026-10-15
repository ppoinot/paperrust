import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} Ko`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
}

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (ch) => {
    if (ch === "&") return String.fromCharCode(38) + "#38;";
    if (ch === "<") return String.fromCharCode(38) + "#60;";
    if (ch === ">") return String.fromCharCode(38) + "#62;";
    if (ch === '"') return String.fromCharCode(38) + "#34;";
    return String.fromCharCode(38) + "#39;";
  });
}
