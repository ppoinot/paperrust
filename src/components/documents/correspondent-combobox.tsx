import { Check } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Input } from "@/components/ui/input";
import type { Correspondent } from "@/lib/paperless/types";
import { cn } from "@/lib/utils";

function norm(s: string) {
  return s.trim().toLowerCase();
}

/**
 * Correspondant combobox — filter existing names, clear via « Aucun »,
 * or keep an unknown draft name for create-on-save by the parent fiche.
 */
export function CorrespondentCombobox({
  correspondents,
  valueId,
  draftName = null,
  disabled,
  onCommit,
}: {
  correspondents: Correspondent[];
  valueId: number | null;
  /** Pending name when the typed value is not yet an existing correspondent. */
  draftName?: string | null;
  disabled?: boolean;
  onCommit: (name: string) => Promise<void> | void;
}) {
  const selected = correspondents.find((c) => c.id === valueId) ?? null;
  const display = selected?.name ?? draftName ?? "";
  const [text, setText] = useState(display);
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState<number | null>(null);
  const [pending, setPending] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const arrowed = useRef(false);

  useEffect(() => {
    setText(display);
  }, [display]);

  const q = text.trim().toLowerCase();
  const filtered = useMemo(
    () =>
      q
        ? correspondents.filter((c) => c.name.toLowerCase().includes(q))
        : correspondents,
    [correspondents, q],
  );
  const unknown = Boolean(text.trim()) && !correspondents.some((c) => norm(c.name) === q);

  async function commitName(name: string) {
    if (pending) return;
    setPending(true);
    arrowed.current = false;
    try {
      await onCommit(name);
      setOpen(false);
      setHi(null);
    } finally {
      setPending(false);
    }
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      arrowed.current = true;
      setOpen(true);
      setHi((h) => {
        const max = filtered.length - 1;
        if (max < 0) return null;
        return h === null ? 0 : Math.min(max, h + 1);
      });
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      arrowed.current = true;
      setHi((h) => {
        if (filtered.length === 0) return null;
        if (h === null || h <= 0) return 0;
        return h - 1;
      });
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (arrowed.current && hi !== null && filtered[hi]) void commitName(filtered[hi].name);
      else void commitName(text);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
      setHi(null);
      arrowed.current = false;
      setText(display);
    }
  }

  return (
    <div ref={wrap} className="relative mt-1">
      <Input
        role="combobox"
        aria-expanded={open}
        aria-autocomplete="list"
        aria-controls="correspondent-combobox-list"
        autoComplete="off"
        value={text}
        disabled={disabled || pending}
        placeholder="Saisir un correspondant…"
        onChange={(e) => {
          arrowed.current = false;
          setText(e.target.value);
          setOpen(true);
          setHi(null);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => {
          window.setTimeout(() => {
            if (!wrap.current?.contains(document.activeElement)) {
              setOpen(false);
              setHi(null);
              arrowed.current = false;
              setText(display);
            }
          }, 120);
        }}
        onKeyDown={onKey}
      />
      {open ? (
        <ul
          id="correspondent-combobox-list"
          role="listbox"
          className="absolute z-50 mt-1 max-h-56 w-full overflow-auto rounded-lg bg-surface p-1 shadow-[var(--shadow-border)]"
        >
          <li>
            <button
              type="button"
              className="flex w-full rounded-md px-2.5 py-2 text-left text-sm text-muted hover:bg-surface-2"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => void commitName("")}
            >
              Aucun
            </button>
          </li>
          {filtered.map((c, i) => (
            <li key={c.id}>
              <button
                type="button"
                role="option"
                aria-selected={c.id === valueId}
                className={cn(
                  "flex w-full items-center justify-between gap-2 rounded-md px-2.5 py-2 text-left text-sm hover:bg-surface-2",
                  hi === i && "bg-surface-2",
                  c.id === valueId && "font-medium",
                )}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => {
                  arrowed.current = true;
                  setHi(i);
                }}
                onClick={() => void commitName(c.name)}
              >
                <span>{c.name}</span>
                {c.id === valueId ? <Check className="size-3.5 shrink-0" /> : null}
              </button>
            </li>
          ))}
          {unknown ? (
            <li>
              <button
                type="button"
                className="flex w-full rounded-md px-2.5 py-2 text-left text-sm text-muted hover:bg-surface-2"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void commitName(text)}
              >
                Créer « {text.trim()} » à l’enregistrement
              </button>
            </li>
          ) : null}
          {filtered.length === 0 && !unknown ? (
            <li className="px-2.5 py-2 text-sm text-muted">Aucun correspondant</li>
          ) : null}
        </ul>
      ) : null}
      {draftName && !valueId ? (
        <p className="mt-1 text-xs text-muted">
          « {draftName} » sera créé à l’enregistrement
        </p>
      ) : null}
    </div>
  );
}
