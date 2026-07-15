// Чат-скролл на @shadcn/react MessageScroller (headless), стилизованный под
// sql-kai. Поведение (docs shadcn "Message Scroller"): новый ход пользователя
// якорится у верха вьюпорта с подглядыванием предыдущего хода, ответ стримится
// в место под ним; follow-output работает только на «живом краю» и отпускается
// любым скроллом читателя; при переоткрытии панель встаёт на последний якорь
// (последний вопрос), а не на абсолютный низ.
import { ArrowDown } from "lucide-react";
import type { ReactNode } from "react";
import { MessageScroller } from "@shadcn/react/message-scroller";
import { cn } from "../ui";

export function Conversation({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <MessageScroller.Provider
      autoScroll
      defaultScrollPosition="last-anchor"
      scrollPreviousItemPeek={48}
    >
      <MessageScroller.Root className={cn("relative min-h-0 flex-1", className)}>
        <MessageScroller.Viewport className="h-full overflow-y-auto outline-none">
          <MessageScroller.Content className="flex min-h-full flex-col gap-2 px-2.5 py-2">
            {children}
          </MessageScroller.Content>
        </MessageScroller.Viewport>
        <MessageScroller.Button
          direction="end"
          title="Scroll to latest"
          className={cn(
            "absolute bottom-2 left-1/2 -translate-x-1/2 rounded-full border p-1.5",
            "border-zinc-700 bg-zinc-900/95 text-zinc-300 shadow-md",
            "hover:border-zinc-600 hover:text-zinc-100",
            "data-[active=false]:hidden",
          )}
        >
          <ArrowDown size={13} />
        </MessageScroller.Button>
      </MessageScroller.Root>
    </MessageScroller.Provider>
  );
}

/** Строка ленты: каждый прямой ребёнок Conversation оборачивается сюда, чтобы
 *  скроллер мог мерить, якорить и адресовать её. `anchor` — строка начинает
 *  новый ход (в чате агента это сообщение пользователя). */
export function ConversationItem({
  id,
  anchor,
  className,
  children,
}: {
  id?: string;
  anchor?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <MessageScroller.Item
      messageId={id}
      scrollAnchor={anchor}
      className={cn("flex flex-col", className)}
    >
      {children}
    </MessageScroller.Item>
  );
}
