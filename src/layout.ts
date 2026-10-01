/**
 * Where a new worker pane goes inside the PM's tab. Pure: it reads pane sizes
 * (in terminal cells) and returns the pane to split and the direction, or
 * nothing when no split leaves both halves at the minimum size.
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
}

export interface PlacementLimits {
  readonly split: "auto" | SplitDirection;
  readonly minColumns: number;
  readonly minRows: number;
}

/** Terminal cells are about twice as tall as wide, so a pane is wide when it has twice as many columns as rows. */
const CELL_ASPECT = 2;

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

function fits(
  pane: LayoutPane,
  direction: SplitDirection,
  limits: PlacementLimits,
): boolean {
  const columns =
    direction === "right" ? Math.floor(pane.width / 2) : pane.width;
  const rows = direction === "down" ? Math.floor(pane.height / 2) : pane.height;
  return columns >= limits.minColumns && rows >= limits.minRows;
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
  limits: PlacementLimits,
): Placement | undefined {
  const candidates = panes
    .filter(usable)
    .sort(
      (a, b) =>
        b.width * b.height - a.width * a.height ||
        comparePaneIds(a.paneId, b.paneId),
    );
  for (const pane of candidates) {
    const directions: SplitDirection[] =
      limits.split !== "auto"
        ? [limits.split]
        : pane.width >= CELL_ASPECT * pane.height
          ? ["right", "down"]
          : ["down", "right"];
    for (const direction of directions)
      if (fits(pane, direction, limits))
        return { targetPaneId: pane.paneId, direction };
  }
  return undefined;
}
