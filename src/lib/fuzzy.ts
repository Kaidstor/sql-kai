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
    const idx = hay.indexOf(token);
    if (idx >= 0) {
      score += 100 - Math.min(idx, 50);
      if (idx === 0 || /[\s\-_./(:]/.test(hay[idx - 1])) score += 40;
    } else if (subsequenceIndices(token, hay)) {
      score += 20;
    } else {
      return null;
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
    const idx = hay.indexOf(token);
    if (idx >= 0) {
      for (let i = 0; i < token.length; i++) hits.add(idx + i);
    } else {
      const seq = subsequenceIndices(token, hay);
      if (seq) for (const i of seq) hits.add(i);
    }
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
