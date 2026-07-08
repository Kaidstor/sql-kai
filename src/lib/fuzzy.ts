/**
 * Token fuzzy match: every whitespace-separated query token must hit the
 * haystack — as a substring (scored by position, word-start bonus) or at
 * least as a subsequence. Returns null when some token doesn't match.
 *
 * "m s dev" matches "ms-search dev …" but not "ms-search prod …".
 */
const tokenize = (query: string) =>
  query.toLowerCase().split(/\s+/).filter(Boolean);

export function fuzzyScore(query: string, haystack: string): number | null {
  const tokens = tokenize(query);
  if (tokens.length === 0) return 0;
  const hay = haystack.toLowerCase();
  let score = 0;
  for (const token of tokens) {
    const m = matchToken(token, hay);
    if (!m) return null;
    if (m.start >= 0) {
      score += 100 - Math.min(m.start, 50);
      if (m.start === 0 || /[\s\-_./(:]/.test(hay[m.start - 1])) score += 40;
    } else {
      score += 20; // subsequence-only match
    }
  }
  return score;
}

/**
 * Splits `text` into runs for match highlighting: chars hit by any query
 * token (same matching as fuzzyScore — first substring occurrence, else
 * subsequence) get `hit: true`. Tokens that don't match `text` are ignored,
 * so this works per displayed field while scoring runs on the combined
 * haystack.
 */
export function highlightRuns(
  query: string,
  text: string,
): { text: string; hit: boolean }[] {
  const tokens = tokenize(query);
  const hay = text.toLowerCase();
  const hits = new Set<number>();
  for (const token of tokens) {
    const m = matchToken(token, hay);
    if (m) for (const i of m.indices) hits.add(i);
  }
  if (hits.size === 0) return [{ text, hit: false }];
  const runs: { text: string; hit: boolean }[] = [];
  let cur = "";
  let curHit = hits.has(0);
  for (let i = 0; i < text.length; i++) {
    const hit = hits.has(i);
    if (hit !== curHit) {
      runs.push({ text: cur, hit: curHit });
      cur = "";
      curHit = hit;
    }
    cur += text[i];
  }
  if (cur) runs.push({ text: cur, hit: curHit });
  return runs;
}

/**
 * How `token` matches `hay` (already lowercased), shared by scoring and
 * highlighting so the two never diverge: a substring match reports its `start`
 * and the contiguous run of indices; a subsequence match reports `start: -1`
 * and the scattered indices; no match is null.
 */
function matchToken(
  token: string,
  hay: string,
): { start: number; indices: number[] } | null {
  const idx = hay.indexOf(token);
  if (idx >= 0) {
    const indices: number[] = [];
    for (let i = 0; i < token.length; i++) indices.push(idx + i);
    return { start: idx, indices };
  }
  const seq = subsequenceIndices(token, hay);
  return seq ? { start: -1, indices: seq } : null;
}

function subsequenceIndices(needle: string, hay: string): number[] | null {
  const out: number[] = [];
  let i = 0;
  for (let j = 0; j < hay.length && i < needle.length; j++) {
    if (hay[j] === needle[i]) {
      out.push(j);
      i++;
    }
  }
  return i >= needle.length ? out : null;
}
