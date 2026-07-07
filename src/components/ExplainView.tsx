import { X } from "lucide-react";
import { useMemo } from "react";
import type { ExplainResult, PlanNode } from "../lib/types";
import { IconBtn, cn } from "./ui";

/** One flattened row of the plan tree with derived metrics. */
interface FlatNode {
  node: PlanNode;
  depth: number;
  /** Total time (ms) spent in this node and its children (loops included). */
  inclusive: number;
  /** inclusive minus the children — what the node itself cost. */
  exclusive: number;
  /** Actual rows produced across all loops (analyze only). */
  actualRows?: number;
}

/** time × loops = node total; EXPLAIN reports per-loop averages. */
const inclusiveOf = (n: PlanNode, analyzed: boolean): number => {
  if (analyzed) {
    return (n["Actual Total Time"] ?? 0) * (n["Actual Loops"] ?? 1);
  }
  return n["Total Cost"] ?? 0;
};

/** Pre-order walk building the display list (parent before children). */
function walk(
  node: PlanNode,
  analyzed: boolean,
  depth: number,
  out: FlatNode[],
) {
  const inclusive = inclusiveOf(node, analyzed);
  const childInclusive = (node.Plans ?? []).reduce(
    (acc, c) => acc + inclusiveOf(c, analyzed),
    0,
  );
  out.push({
    node,
    depth,
    inclusive,
    exclusive: Math.max(0, inclusive - childInclusive),
    actualRows:
      node["Actual Rows"] !== undefined
        ? node["Actual Rows"] * (node["Actual Loops"] ?? 1)
        : undefined,
  });
  for (const child of node.Plans ?? []) {
    walk(child, analyzed, depth + 1, out);
  }
}

const fmtNum = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${(n / 1000).toFixed(0)}k`;
  return String(Math.round(n * 100) / 100);
};

const fmtMs = (ms: number): string =>
  ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms.toFixed(2)} ms`;

/** Node headline: "Index Scan using users_pkey on public.users u". */
function nodeTitle(n: PlanNode): string {
  const parts = [n["Node Type"]];
  if (n["Join Type"] && n["Join Type"] !== "Inner") parts.push(n["Join Type"]);
  if (n["Index Name"]) parts.push(`using ${n["Index Name"]}`);
  if (n["Relation Name"]) {
    const rel =
      n.Schema && n.Schema !== "public"
        ? `${n.Schema}.${n["Relation Name"]}`
        : n["Relation Name"];
    const alias = n.Alias && n.Alias !== n["Relation Name"] ? ` ${n.Alias}` : "";
    parts.push(`on ${rel}${alias}`);
  }
  return parts.join(" ");
}

/** Condition/detail sub-lines shown dimmed under the node title. */
function nodeDetails(n: PlanNode): [string, string][] {
  const out: [string, string][] = [];
  if (n["Index Cond"]) out.push(["Index Cond", n["Index Cond"]]);
  if (n["Recheck Cond"]) out.push(["Recheck Cond", n["Recheck Cond"]]);
  if (n["Hash Cond"]) out.push(["Hash Cond", n["Hash Cond"]]);
  if (n["Merge Cond"]) out.push(["Merge Cond", n["Merge Cond"]]);
  if (n.Filter) out.push(["Filter", n.Filter]);
  if (n["Sort Key"]?.length) {
    out.push([
      n["Sort Method"] ? `Sort (${n["Sort Method"]})` : "Sort Key",
      n["Sort Key"].join(", "),
    ]);
  }
  return out;
}

export function ExplainView({
  explain,
  onClose,
}: {
  explain: ExplainResult;
  onClose: () => void;
}) {
  const { analyzed } = explain;
  const rows = useMemo(() => {
    const out: FlatNode[] = [];
    walk(explain.Plan, analyzed, 0, out);
    return out;
  }, [explain, analyzed]);

  // Heat is a node's own (exclusive) share of the whole plan.
  const total = Math.max(
    rows[0]?.inclusive ?? 0,
    rows.reduce((acc, r) => acc + r.exclusive, 0),
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-3 border-b border-zinc-800 px-3 py-1.5 text-[11px] text-zinc-400">
        <span className="font-semibold text-zinc-200">
          {analyzed ? "EXPLAIN ANALYZE" : "EXPLAIN"}
        </span>
        {explain["Planning Time"] !== undefined && (
          <span>planning {fmtMs(explain["Planning Time"])}</span>
        )}
        {explain["Execution Time"] !== undefined && (
          <span>execution {fmtMs(explain["Execution Time"])}</span>
        )}
        {!analyzed && (
          <span>total cost {fmtNum(explain.Plan["Total Cost"] ?? 0)}</span>
        )}
        <span className="text-zinc-600">
          {analyzed ? "bar = node time share" : "bar = node cost share"}
        </span>
        <div className="ml-auto">
          <IconBtn title="Back to results" onClick={onClose}>
            <X size={13} />
          </IconBtn>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto py-1">
        {rows.map((r, i) => {
          const share = total > 0 ? r.exclusive / total : 0;
          const est = r.node["Plan Rows"] ?? 0;
          const misestimate =
            analyzed &&
            r.actualRows !== undefined &&
            est > 0 &&
            (r.actualRows / est >= 10 || r.actualRows / est <= 0.1) &&
            Math.max(r.actualRows, est) >= 100;
          const removed =
            (r.node["Rows Removed by Filter"] ?? 0) +
            (r.node["Rows Removed by Join Filter"] ?? 0);
          return (
            <div
              key={i}
              className="flex items-start gap-3 px-3 py-1 hover:bg-zinc-900/60"
              style={{
                paddingLeft: `${12 + r.depth * 18}px`,
                background:
                  share > 0.02
                    ? `linear-gradient(to right, rgba(239,68,68,${(
                        0.04 + share * 0.2
                      ).toFixed(3)}), transparent 70%)`
                    : undefined,
              }}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2">
                  <span className="text-[12px] font-medium text-zinc-100">
                    {r.node["Subplan Name"] && (
                      <span className="text-violet-400">
                        {r.node["Subplan Name"]}:{" "}
                      </span>
                    )}
                    {nodeTitle(r.node)}
                  </span>
                  {share >= 0.01 && (
                    <span
                      className={cn(
                        "text-[10px] tabular-nums",
                        share >= 0.3
                          ? "font-semibold text-red-400"
                          : share >= 0.1
                            ? "text-amber-400"
                            : "text-zinc-500",
                      )}
                    >
                      {(share * 100).toFixed(0)}%
                    </span>
                  )}
                  {misestimate && (
                    <span
                      title={`Planner expected ${fmtNum(est)} row(s), got ${fmtNum(
                        r.actualRows ?? 0,
                      )} — statistics may be stale (ANALYZE the table?)`}
                      className="rounded border border-amber-500/40 bg-amber-500/10 px-1 text-[9px] font-semibold text-amber-400"
                    >
                      est off ×
                      {fmtNum(
                        Math.max(
                          (r.actualRows ?? 0) / est,
                          est / Math.max(r.actualRows ?? 0, 1),
                        ),
                      )}
                    </span>
                  )}
                </div>
                {nodeDetails(r.node).map(([label, value]) => (
                  <div
                    key={label}
                    className="truncate font-mono text-[10px] text-zinc-500"
                    title={`${label}: ${value}`}
                  >
                    <span className="text-zinc-600">{label}:</span> {value}
                  </div>
                ))}
                {removed > 0 && (
                  <div className="font-mono text-[10px] text-amber-500/80">
                    rows removed by filter: {fmtNum(removed)}
                  </div>
                )}
              </div>

              <div className="flex shrink-0 items-baseline gap-3 pt-px font-mono text-[10px] tabular-nums text-zinc-500">
                <span title="rows: planner estimate → actual (× loops)">
                  rows {fmtNum(est)}
                  {r.actualRows !== undefined && ` → ${fmtNum(r.actualRows)}`}
                </span>
                {analyzed ? (
                  <span title="time spent in this node incl. children (all loops)">
                    {fmtMs(r.inclusive)}
                  </span>
                ) : (
                  <span title="cost: startup..total">
                    cost {fmtNum(r.node["Startup Cost"] ?? 0)}..
                    {fmtNum(r.node["Total Cost"] ?? 0)}
                  </span>
                )}
                {(r.node["Actual Loops"] ?? 1) > 1 && (
                  <span title="loops">×{fmtNum(r.node["Actual Loops"] ?? 1)}</span>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
