// Vendored shadcn/ui AI "conversation" primitive (shadcn.io/ai), restyled for
// sql-kai. use-stick-to-bottom даёт правильный чат-скролл: прилипание к низу
// на стриме (ResizeObserver, без дёрганий), освобождение при скролле вверх и
// плавающая кнопка «вниз».
import { ArrowDown } from "lucide-react";
import type { ReactNode } from "react";
import { StickToBottom, useStickToBottomContext } from "use-stick-to-bottom";
import { cn } from "../ui";

export function Conversation({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <StickToBottom
      className={cn("relative min-h-0 flex-1 overflow-y-auto", className)}
      initial="instant"
      resize="smooth"
      role="log"
    >
      <StickToBottom.Content className="flex min-h-full flex-col gap-2 px-2.5 py-2">
        {children}
      </StickToBottom.Content>
      <ScrollToBottom />
    </StickToBottom>
  );
}

function ScrollToBottom() {
  const { isAtBottom, scrollToBottom } = useStickToBottomContext();
  if (isAtBottom) return null;
  return (
    <button
      type="button"
      title="Scroll to bottom"
      onClick={() => void scrollToBottom()}
      className={cn(
        "absolute bottom-2 left-1/2 -translate-x-1/2 rounded-full border p-1.5",
        "border-zinc-700 bg-zinc-900/95 text-zinc-300 shadow-md",
        "hover:border-zinc-600 hover:text-zinc-100",
      )}
    >
      <ArrowDown size={13} />
    </button>
  );
}
