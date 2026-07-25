/** Layout-independent hotkey matching.
 *
 *  Non-Latin layouts (Cyrillic, Greek, …) report `e.key` as the local
 *  character — "с" for the C key — so combos matched by character never fire
 *  there. When the typed character is not a Latin letter, match the physical
 *  position (`e.code`) instead. Latin layouts keep matching by character, so
 *  on Dvorak/AZERTY the combo follows the key cap, not the QWERTY position.
 */
export const isKey = (
  e: { key: string; code: string },
  letter: string,
): boolean => {
  const k = e.key.toLowerCase();
  if (/^[a-z]$/.test(k)) return k === letter;
  return e.code === `Key${letter.toUpperCase()}`;
};

/** Digit of the pressed key (top row or numpad), or null. By position first:
 *  layouts where digits live on shifted keys (AZERTY) or a dead character
 *  still report the physical Digit1…9. */
export const keyDigit = (e: { key: string; code: string }): number | null => {
  const m = /^(?:Digit|Numpad)([0-9])$/.exec(e.code);
  if (m) return Number(m[1]);
  return /^[0-9]$/.test(e.key) ? Number(e.key) : null;
};
