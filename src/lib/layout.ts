/**
 * Wrong-keyboard-layout rescue for search fields: "зкщв" typed on ЙЦУКЕН is
 * "prod" on QWERTY. `switchLayout` re-types a string in the opposite layout
 * (both directions, per character), leaving unmapped chars untouched.
 *
 * Expects lowercased input — the maps only cover lowercase keys.
 */
const RU_TO_EN: Record<string, string> = {
  й: "q", ц: "w", у: "e", к: "r", е: "t", н: "y", г: "u", ш: "i",
  щ: "o", з: "p", х: "[", ъ: "]", ф: "a", ы: "s", в: "d", а: "f",
  п: "g", р: "h", о: "j", л: "k", д: "l", ж: ";", э: "'", я: "z",
  ч: "x", с: "c", м: "v", и: "b", т: "n", ь: "m", б: ",", ю: ".",
  ё: "`",
};

const EN_TO_RU: Record<string, string> = Object.fromEntries(
  Object.entries(RU_TO_EN).map(([ru, en]) => [en, ru]),
);

export function switchLayout(s: string): string {
  let out = "";
  for (const ch of s) out += RU_TO_EN[ch] ?? EN_TO_RU[ch] ?? ch;
  return out;
}
