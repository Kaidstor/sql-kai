/** One visual line of a cell value: newlines collapse to spaces so every row
 *  keeps the uniform height the body windowing assumes (useRowWindow). The
 *  full value stays reachable via the cell title and the ⌘⏎ dialog. */
export function oneLine(v: string): string {
  return v.includes("\n") || v.includes("\r")
    ? v.replace(/\r\n|[\n\r]/g, " ")
    : v;
}
