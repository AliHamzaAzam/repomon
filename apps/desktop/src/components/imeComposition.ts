/** Chrome/Firefox expose composition directly; WebKit can end it before the confirming keydown. */
export function isImeConfirmation(event: Pick<KeyboardEvent, "isComposing" | "keyCode">): boolean {
  // MDN still recommends deprecated keyCode 229 for IME keydowns whose isComposing is false.
  return event.isComposing || event.keyCode === 229;
}
