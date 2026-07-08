import clsx from "clsx";
import { Check, ChevronDown, Loader2, RefreshCw } from "lucide-react";
import { useEffect, useRef } from "react";
import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  LabelHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
} from "react";

export const cn = clsx;

/** Full-screen dimmed backdrop; a mousedown on it (not its content) closes.
 *  Opt into `closeOnEsc` for dialogs that should also dismiss on Escape —
 *  saves each consumer hand-rolling the same keydown listener. */
export function Overlay({
  onClose,
  className,
  closeOnEsc = false,
  children,
}: {
  onClose: () => void;
  /** Vertical alignment + dim level, e.g. "items-center bg-black/60". */
  className?: string;
  closeOnEsc?: boolean;
  children: ReactNode;
}) {
  useEffect(() => {
    if (!closeOnEsc) return;
    // Capture phase + stopPropagation so an open grid/editor underneath the
    // dialog doesn't also react to the same Escape.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [closeOnEsc, onClose]);
  return (
    <div
      className={cn("fixed inset-0 z-50 flex justify-center", className)}
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      {children}
    </div>
  );
}

/** Dropdown anchored to its trigger; closes on outside click / Esc. */
export function Popover({
  open,
  onClose,
  trigger,
  children,
  side = "bottom",
  panelClassName,
}: {
  open: boolean;
  onClose: () => void;
  trigger: ReactNode;
  children: ReactNode;
  side?: "bottom" | "top";
  panelClassName?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);

  return (
    <div ref={ref} className="relative">
      {trigger}
      {open && (
        <div
          className={cn(
            "absolute left-0 z-40 rounded-md border border-zinc-700",
            "bg-zinc-900 shadow-xl shadow-black/40",
            side === "bottom" ? "top-full mt-1" : "bottom-full mb-1",
            panelClassName,
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}

type ButtonVariant = "primary" | "ghost" | "danger";

export function Button({
  variant = "ghost",
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant }) {
  return (
    <button
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-[12px] font-medium transition-colors",
        "disabled:opacity-40 disabled:pointer-events-none",
        variant === "primary" &&
          "bg-sky-600 text-white hover:bg-sky-500 active:bg-sky-600",
        variant === "ghost" &&
          "text-zinc-300 hover:bg-zinc-800 active:bg-zinc-700/80",
        variant === "danger" &&
          "bg-red-600/90 text-white hover:bg-red-500 active:bg-red-600",
        className,
      )}
      {...props}
    />
  );
}

/** Toolbar dropdown trigger: icon + label with a trailing chevron. */
export function MenuBtn({
  className,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={cn(
        "flex items-center gap-1 rounded px-1.5 py-1 text-[12px] text-zinc-400",
        "transition-colors hover:bg-zinc-700/60 hover:text-zinc-100",
        className,
      )}
      {...props}
    >
      {children}
      <ChevronDown size={11} className="text-zinc-600" />
    </button>
  );
}

/** Red PROD chip shown wherever a production profile appears. */
export function ProdBadge({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        "shrink-0 rounded border border-red-500/40 bg-red-500/15 px-1 py-px",
        "text-[9px] font-semibold tracking-wide text-red-400",
        className,
      )}
    >
      PROD
    </span>
  );
}

/** Chip shown while kai holds a live broker session for the profile. */
export function CliBadge({ idleSec }: { idleSec?: number | null }) {
  return (
    <span
      title={`Live kai (CLI) session owned by the app${
        idleSec != null ? ` — idle ${idleSec}s` : ""
      }`}
      className={cn(
        "shrink-0 rounded border border-sky-500/40 bg-sky-500/10 px-1 py-px",
        "font-mono text-[9px] font-semibold text-sky-400",
      )}
    >
      cli
    </span>
  );
}

/** Connection accent dot; hollow when the profile has no color. */
export function ColorDot({ color }: { color: string | null }) {
  return (
    <span
      className="size-2.5 shrink-0 rounded-full border border-zinc-600"
      style={
        color ? { background: color, borderColor: "transparent" } : undefined
      }
    />
  );
}

export function IconBtn({
  className,
  title,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      title={title}
      className={cn(
        "inline-flex items-center justify-center rounded p-1 text-zinc-400",
        "hover:bg-zinc-700/60 hover:text-zinc-100 transition-colors",
        "disabled:opacity-40 disabled:pointer-events-none",
        className,
      )}
      {...props}
    />
  );
}

/** Refresh icon button that swaps to a spinner while loading. */
export function RefreshBtn({
  loading,
  onClick,
  title = "Refresh",
}: {
  loading: boolean;
  onClick: () => void;
  title?: string;
}) {
  return (
    <IconBtn title={title} disabled={loading} onClick={onClick}>
      {loading ? (
        <Loader2 size={13} className="animate-spin" />
      ) : (
        <RefreshCw size={13} />
      )}
    </IconBtn>
  );
}

/** Apply/Discard pair shown in a tab toolbar while changes are staged. */
export function PendingChangesBar({
  count,
  busy,
  applyTitle,
  discardTitle,
  onApply,
  onDiscard,
}: {
  count: number;
  busy: boolean;
  /** Tooltip explaining what Apply runs (⌘S hint etc.). */
  applyTitle: string;
  discardTitle?: string;
  onApply: () => void;
  onDiscard: () => void;
}) {
  return (
    <div className="flex items-center gap-1.5 pl-1">
      <Button variant="primary" disabled={busy} onClick={onApply} title={applyTitle}>
        <Check size={13} /> Apply {count}
      </Button>
      <Button title={discardTitle} onClick={onDiscard}>
        Discard
      </Button>
    </div>
  );
}

export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "w-full rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1 text-[13px]",
        "text-zinc-100 placeholder:text-zinc-600",
        "focus:border-sky-600 focus:outline-none",
        className,
      )}
      {...props}
    />
  );
}

export function Select({
  className,
  ...props
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={cn(
        "rounded-md border border-zinc-700 bg-zinc-900 px-1.5 py-0.5 text-[12px]",
        "text-zinc-200 focus:border-sky-600 focus:outline-none",
        className,
      )}
      {...props}
    />
  );
}

export function Label({
  className,
  ...props
}: LabelHTMLAttributes<HTMLLabelElement>) {
  return (
    <label
      className={cn("block text-[11px] text-zinc-400 mb-1", className)}
      {...props}
    />
  );
}

/** Red monospace error block used under query/table/structure views. */
export function ErrorPre({ children }: { children: ReactNode }) {
  return (
    <pre className="selectable m-2 rounded-md border border-red-900/60 bg-red-950/40 p-3 font-mono text-[12px] whitespace-pre-wrap text-red-300">
      {children}
    </pre>
  );
}

export function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={className}>
      <Label>{label}</Label>
      {children}
    </div>
  );
}
