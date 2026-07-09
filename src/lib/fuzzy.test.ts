import { describe, expect, it } from "vitest";
import { fuzzyScore, highlightRuns } from "./fuzzy";

describe("fuzzyScore", () => {
  it("empty query matches everything with score 0", () => {
    expect(fuzzyScore("", "anything")).toBe(0);
    expect(fuzzyScore("   ", "anything")).toBe(0);
  });

  it("null when any token misses", () => {
    expect(fuzzyScore("dev prod", "ms-search dev")).toBeNull();
    expect(fuzzyScore("zzz", "abc")).toBeNull();
  });

  it("every token must hit: the README example", () => {
    expect(fuzzyScore("m s dev", "ms-search dev main")).not.toBeNull();
    expect(fuzzyScore("m s dev", "ms-search prod main")).toBeNull();
  });

  it("earlier and word-start matches score higher", () => {
    const atStart = fuzzyScore("search", "search-api")!;
    const wordStart = fuzzyScore("search", "ms-search")!;
    const midWord = fuzzyScore("search", "xxsearch")!;
    expect(atStart).toBeGreaterThan(midWord);
    expect(wordStart).toBeGreaterThan(midWord);
  });

  it("substring beats subsequence-only", () => {
    const substring = fuzzyScore("abc", "xx abc")!;
    const subsequence = fuzzyScore("abc", "a1b2c3")!;
    expect(substring).toBeGreaterThan(subsequence);
  });

  it("is case-insensitive", () => {
    expect(fuzzyScore("DEV", "ms Dev")).not.toBeNull();
  });
});

describe("highlightRuns", () => {
  it("single non-hit run when nothing matches", () => {
    expect(highlightRuns("zzz", "abc")).toEqual([{ text: "abc", hit: false }]);
    expect(highlightRuns("", "abc")).toEqual([{ text: "abc", hit: false }]);
  });

  it("marks the substring hit and preserves the original text", () => {
    expect(highlightRuns("sea", "ms-Search")).toEqual([
      { text: "ms-", hit: false },
      { text: "Sea", hit: true },
      { text: "rch", hit: false },
    ]);
  });

  it("subsequence hits are scattered", () => {
    expect(highlightRuns("ac", "abc")).toEqual([
      { text: "a", hit: true },
      { text: "b", hit: false },
      { text: "c", hit: true },
    ]);
  });

  it("tokens that do not match this field are ignored", () => {
    // scoring runs on a combined haystack; per-field highlighting must not
    // require every token to hit
    expect(highlightRuns("dev abc", "abc")).toEqual([{ text: "abc", hit: true }]);
  });

  it("runs concatenate back to the input", () => {
    const runs = highlightRuns("a c", "abcabc");
    expect(runs.map((r) => r.text).join("")).toBe("abcabc");
  });
});
