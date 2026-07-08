/** True on macOS — drives ⌘-vs-Ctrl labels and the traffic-light title-bar
 *  inset. Evaluated once; the renderer always has `navigator`. */
export const isMac = navigator.userAgent.includes("Mac");
