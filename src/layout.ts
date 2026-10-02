/**
 * Where a new worker pane goes inside the PM's tab. Pure: it reads pane sizes
 * (in terminal cells) and returns the pane to split, the direction and the
 * share the target keeps, or nothing when no split leaves both parts at the
 * minimum size. The PM pane stays on the left at full height: the first worker
 * is split off to its right, later workers stack down in that column, and the
 * PM pane is never split down.
 */

export interface LayoutPane {
  readonly paneId: string;
  readonly width: number;
  readonly height: number;
}

export type SplitDirection = "right" | "down";

export interface Placement {
  readonly targetPaneId: string;
  readonly direction: SplitDirection;
  /** The fraction of the target pane that the target keeps. */
  readonly keep: number;
}

export interface PlacementLimits {
  readonly pmWidthPercent: number;
  readonly minColumns: number;
  readonly minRows: number;
}

/** Compares ids such as w2:p10 and w2:p2 by their numeric parts (any length, leading zeros allowed), so p10 sorts after p2; equal numbers fall back to the plain text, so the order is always fixed. */
export function comparePaneIds(a: string, b: string): number {
  const parts = (id: string): Array<string | bigint> =>
    id.split(/(\d+)/).map((part) => (/^\d+$/.test(part) ? BigInt(part) : part));
  const left = parts(a);
  const right = parts(b);
  for (let index = 0; index < Math.min(left.length, right.length); index += 1) {
    const x = left[index]!;
    const y = right[index]!;
    if (x === y) continue;
    if (typeof x === "bigint" && typeof y === "bigint") return x < y ? -1 : 1;
    return String(x) < String(y) ? -1 : 1;
  }
  return left.length - right.length || (a < b ? -1 : a > b ? 1 : 0);
}

function usable(pane: LayoutPane): boolean {
  return (
    Number.isSafeInteger(pane.width) &&
    Number.isSafeInteger(pane.height) &&
    pane.width > 0 &&
    pane.height > 0
  );
}

export function choosePlacement(
  panes: readonly LayoutPane[],
  pmPaneId: string,
  limits: PlacementLimits,
): Placement | undefined {
  const pm = panes.find((pane) => pane.paneId === pmPaneId);
  if (pm === undefined || !usable(pm)) return undefined;
  const workers = panes
    .filter((pane) => pane.paneId !== pmPaneId && usable(pane))
    .sort((a, b) => b.height - a.height || comparePaneIds(a.paneId, b.paneId));
  if (workers.length === 0) {
    const keep = limits.pmWidthPercent / 100;
    const kept = Math.floor(pm.width * keep);
    const given = pm.width - kept;
    return kept >= limits.minColumns && given >= limits.minColumns
      ? { targetPaneId: pm.paneId, direction: "right", keep }
      : undefined;
  }
  for (const pane of workers)
    if (
      pane.width >= limits.minColumns &&
      Math.floor(pane.height / 2) >= limits.minRows
    )
      return { targetPaneId: pane.paneId, direction: "down", keep: 0.5 };
  return undefined;
}
