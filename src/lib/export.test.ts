import { describe, expect, it } from "vitest";
import { toCsv, toTsv } from "./export";

describe("toTsv", () => {
  it("joins cells with tabs and rows with newlines", () => {
    expect(toTsv(["id", "name"], [["1", "Alice"]])).toBe("id\tname\n1\tAlice");
  });

  it("renders NULL as an empty field", () => {
    expect(toTsv(["a", "b"], [[null, "x"]])).toBe("a\tb\n\tx");
  });

  it("quotes cells containing tabs, newlines or quotes", () => {
    expect(
      toTsv(
        ["id", "note"],
        [
          ["1", "a\tb"],
          ["2", "x\ny"],
          ["3", 'he said "hi"'],
        ],
      ),
    ).toBe('id\tnote\n1\t"a\tb"\n2\t"x\ny"\n3\t"he said ""hi"""');
  });
});

describe("toCsv", () => {
  it("quotes separators and doubles quotes (RFC 4180)", () => {
    expect(toCsv(["a", "b"], [['x,"y"', null]])).toBe('a,b\r\n"x,""y""",');
  });
});
