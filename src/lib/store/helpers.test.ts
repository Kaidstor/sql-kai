import { describe, expect, it } from "vitest";
import { nextQueryTitle } from "./helpers";
import type { Tab } from "./types";

/** Open tabs with the given titles — the only field the helper reads. */
const tabs = (...titles: string[]) => ({
  tabs: titles.map((title) => ({ title }) as Tab),
});

describe("nextQueryTitle", () => {
  it("starts at 1 with no query tabs open", () => {
    expect(nextQueryTitle(tabs())).toBe("Query 1");
    expect(nextQueryTitle(tabs("users", "public.orders ⚙"))).toBe("Query 1");
  });

  it("fills gaps with the smallest free number", () => {
    expect(nextQueryTitle(tabs("Query 2", "Query 5"))).toBe("Query 1");
    expect(nextQueryTitle(tabs("Query 1", "Query 2", "Query 5"))).toBe(
      "Query 3",
    );
  });

  it("ignores renamed and non-matching titles", () => {
    expect(nextQueryTitle(tabs("Query 1 (old)", "my query"))).toBe("Query 1");
  });
});
