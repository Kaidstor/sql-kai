import { describe, expect, it } from "vitest";
import { switchLayout } from "./layout";

describe("switchLayout", () => {
  it("converts ЙЦУКЕН input to QWERTY", () => {
    expect(switchLayout("зкщв")).toBe("prod");
    expect(switchLayout("дщсфдрщые")).toBe("localhost");
    expect(switchLayout("ыйд-лфш")).toBe("sql-kai");
  });

  it("converts QWERTY input to ЙЦУКЕН", () => {
    expect(switchLayout("ghjl")).toBe("прод");
  });

  it("keeps unmapped chars (digits, dots, dashes) as-is", () => {
    expect(switchLayout("зкщв-5432")).toBe("prod-5432");
    expect(switchLayout("127.0.0.1")).toBe("127ю0ю0ю1");
  });

  it("returns empty string for empty input", () => {
    expect(switchLayout("")).toBe("");
  });
});
