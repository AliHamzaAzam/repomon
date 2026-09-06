/// Moves an item around a target and returns null for a no-op so callers can skip persistence.
export function reorderAround<T extends string | number>(
  ids: T[],
  dragged: T,
  target: T,
  after: boolean,
): T[] | null {
  if (dragged === target) return null;
  const without = ids.filter((id) => id !== dragged);
  let at = without.indexOf(target);
  if (at < 0) return null;
  if (after) at += 1;
  without.splice(at, 0, dragged);
  return without;
}
