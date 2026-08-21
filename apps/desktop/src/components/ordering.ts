/// Move `dragged` to sit just `after` (or before) `target` in `ids`, returning the new order
/// (a no-op returns null so callers can skip the RPC). Pure so the reorder math is unit-testable.
///
/// Shared by every drag-to-reorder surface (repo sidebar headers, per-lane agent tabs).
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
