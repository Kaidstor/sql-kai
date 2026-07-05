import { ContextMenu as BaseMenu } from "@base-ui/react/context-menu";
import type { ComponentProps } from "react";
import { cn } from "./ui";

export const ContextMenu = BaseMenu.Root;
export const ContextMenuTrigger = BaseMenu.Trigger;

export function ContextMenuContent({
  className,
  children,
  ...props
}: ComponentProps<typeof BaseMenu.Popup>) {
  return (
    <BaseMenu.Portal>
      <BaseMenu.Positioner className="z-50 outline-none">
        <BaseMenu.Popup
          className={cn(
            "min-w-48 rounded-md border border-zinc-700 bg-zinc-900 p-1",
            "text-[12px] text-zinc-200 shadow-xl shadow-black/50 outline-none",
            "transition-[opacity,transform] duration-100",
            "data-[starting-style]:scale-95 data-[starting-style]:opacity-0",
            "data-[ending-style]:opacity-0",
            className,
          )}
          {...props}
        >
          {children}
        </BaseMenu.Popup>
      </BaseMenu.Positioner>
    </BaseMenu.Portal>
  );
}

export function ContextMenuItem({
  className,
  ...props
}: ComponentProps<typeof BaseMenu.Item>) {
  return (
    <BaseMenu.Item
      className={cn(
        "flex select-none items-center gap-2 rounded px-2 py-1 outline-none",
        "cursor-default text-zinc-200",
        "data-highlighted:bg-zinc-700/60 data-highlighted:text-zinc-50",
        "data-disabled:pointer-events-none data-disabled:opacity-40",
        className,
      )}
      {...props}
    />
  );
}

export function ContextMenuSeparator({
  className,
  ...props
}: ComponentProps<typeof BaseMenu.Separator>) {
  return (
    <BaseMenu.Separator
      className={cn("my-1 h-px bg-zinc-800", className)}
      {...props}
    />
  );
}

export function ContextMenuShortcut({
  className,
  ...props
}: ComponentProps<"span">) {
  return (
    <span
      className={cn("ml-auto pl-4 text-[10px] text-zinc-500", className)}
      {...props}
    />
  );
}
