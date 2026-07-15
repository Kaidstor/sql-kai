// Vendored shadcn/ui AI "response" primitive, restyled: markdown ответов
// агента в компактной zinc-палитре панели (12px, тёмные код-блоки, gfm-таблицы
// со скроллом). memo — стрим дописывает последний элемент, остальные не должны
// перерендериваться.
import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "../ui";

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="selectable min-w-0 text-[12px] leading-relaxed text-zinc-200">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          p: ({ children }) => <p className="my-1 first:mt-0 last:mb-0">{children}</p>,
          a: ({ children, href }) => (
            <a
              href={href}
              target="_blank"
              rel="noreferrer"
              className="text-sky-400 underline decoration-sky-400/40 hover:decoration-sky-400"
            >
              {children}
            </a>
          ),
          ul: ({ children }) => (
            <ul className="my-1 list-disc space-y-0.5 pl-4">{children}</ul>
          ),
          ol: ({ children }) => (
            <ol className="my-1 list-decimal space-y-0.5 pl-4">{children}</ol>
          ),
          h1: Heading,
          h2: Heading,
          h3: Heading,
          h4: Heading,
          blockquote: ({ children }) => (
            <blockquote className="my-1 border-l-2 border-zinc-700 pl-2 text-zinc-400">
              {children}
            </blockquote>
          ),
          code: ({ children, className }) => {
            // block-код приходит с className="language-…", инлайн — без
            const block = /language-/.test(className ?? "");
            return block ? (
              <code className={cn("block font-mono text-[11px]", className)}>
                {children}
              </code>
            ) : (
              <code className="rounded bg-zinc-800 px-1 py-px font-mono text-[11px] text-zinc-100">
                {children}
              </code>
            );
          },
          pre: ({ children }) => (
            <pre className="my-1.5 overflow-x-auto rounded-md border border-zinc-800 bg-zinc-900 p-2">
              {children}
            </pre>
          ),
          table: ({ children }) => (
            <div className="my-1.5 overflow-x-auto">
              <table className="w-full border-collapse text-[11px]">{children}</table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 text-left font-medium text-zinc-300">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="border border-zinc-800 px-1.5 py-0.5 text-zinc-300">
              {children}
            </td>
          ),
          hr: () => <hr className="my-2 border-zinc-800" />,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});

function Heading({ children }: { children?: React.ReactNode }) {
  return (
    <div className="mt-2 mb-1 text-[12px] font-semibold text-zinc-100 first:mt-0">
      {children}
    </div>
  );
}
