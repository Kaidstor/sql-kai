import { describe, expect, it } from "vitest";
import { isKey, keyDigit } from "./keys";

describe("isKey", () => {
  it("matches by character on Latin layouts", () => {
    expect(isKey({ key: "c", code: "KeyC" }, "c")).toBe(true);
    expect(isKey({ key: "C", code: "KeyC" }, "c")).toBe(true);
    // Dvorak: physical KeyI types "c" — the key cap wins
    expect(isKey({ key: "c", code: "KeyI" }, "c")).toBe(true);
    expect(isKey({ key: "b", code: "KeyN" }, "c")).toBe(false);
  });

  it("falls back to the physical position on non-Latin layouts", () => {
    // Russian ЙЦУКЕН: the C key types "с", X types "ч"
    expect(isKey({ key: "с", code: "KeyC" }, "c")).toBe(true);
    expect(isKey({ key: "ч", code: "KeyX" }, "x")).toBe(true);
    expect(isKey({ key: "ч", code: "KeyX" }, "c")).toBe(false);
  });

  it("does not treat a Latin character as a position match", () => {
    // Dvorak: physical KeyC types "j" — must not fire the "c" hotkey
    expect(isKey({ key: "j", code: "KeyC" }, "c")).toBe(false);
  });
});

describe("keyDigit", () => {
  it("reads the physical top-row and numpad digits", () => {
    expect(keyDigit({ key: "1", code: "Digit1" })).toBe(1);
    expect(keyDigit({ key: "9", code: "Numpad9" })).toBe(9);
    // AZERTY: unshifted Digit1 types "&"
    expect(keyDigit({ key: "&", code: "Digit1" })).toBe(1);
  });

  it("falls back to the character, null otherwise", () => {
    expect(keyDigit({ key: "5", code: "" })).toBe(5);
    expect(keyDigit({ key: "a", code: "KeyA" })).toBeNull();
  });
});
