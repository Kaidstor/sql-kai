import { useEffect, useState } from "react";
import { useApp } from "../lib/store";
import { sqlPreview } from "../lib/sql";
import { Button, Field, Input, Overlay } from "./ui";

/** Values for a query's `$1..$N` before it runs, prefilled from the last run
 *  (they live in the vault, see store::remember_params). Enter runs, Esc
 *  cancels. */
export function ParamsDialog() {
  const prompt = useApp((s) => s.paramsPrompt);
  const close = useApp((s) => s.closeParamsPrompt);
  const submit = useApp((s) => s.submitParamsPrompt);
  const forget = useApp((s) => s.forgetParamsPrompt);
  const [values, setValues] = useState<string[]>([]);

  useEffect(() => {
    if (prompt) setValues(prompt.values);
  }, [prompt]);

  if (!prompt) return null;

  const setAt = (i: number, v: string) =>
    setValues((prev) => prev.map((old, j) => (j === i ? v : old)));

  const run = () => {
    void submit(Array.from({ length: prompt.count }, (_, i) => values[i] ?? ""));
  };

  return (
    <Overlay onClose={close} closeOnEsc className="items-center bg-black/60">
      <div className="w-120 rounded-lg border border-zinc-700 bg-zinc-900 p-4 shadow-2xl">
        <div className="text-[13px] font-semibold text-zinc-100">
          {prompt.action.kind === "explain" ? "Explain with parameters" : "Run with parameters"}
        </div>
        <div className="selectable mt-1.5 max-h-24 overflow-y-auto rounded border border-zinc-800 bg-zinc-950 px-2 py-1.5 font-mono text-[11px] leading-relaxed text-zinc-400">
          {sqlPreview(prompt.sql)}
        </div>
        <div
          className="mt-3 flex flex-col gap-2"
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              run();
            }
          }}
        >
          {Array.from({ length: prompt.count }, (_, i) => (
            <Field key={i} label={`$${i + 1}`}>
              <Input
                autoFocus={i === 0}
                value={values[i] ?? ""}
                onChange={(e) => setAt(i, e.target.value)}
                placeholder="value"
              />
            </Field>
          ))}
        </div>
        <div className="mt-2 text-[11px] leading-relaxed text-zinc-500">
          {prompt.remembered
            ? "Filled in from the last run — kept in the vault, never in the query history."
            : "Values are escaped as literals and kept in the vault for next time, not in the query history."}
        </div>
        <div className="mt-4 flex justify-end gap-2">
          {prompt.remembered && (
            <Button className="mr-auto" onClick={() => void forget()}>
              Forget saved
            </Button>
          )}
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" onClick={run}>
            {prompt.action.kind === "explain" ? "Explain" : "Run"}
          </Button>
        </div>
      </div>
    </Overlay>
  );
}
