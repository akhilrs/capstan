import { buildDashModel, type DashModel } from "../src/dash/model.js";
import type { ViewState } from "../src/dash/view.js";
import { NOW, showcase, showcaseRings } from "./dash-fixtures.js";

export function modelOf(
  status = showcase(),
  limit: number | null = 4,
): DashModel {
  return buildDashModel(status, NOW, limit);
}

export function viewOf(
  columns: number,
  rows: number,
  overrides: Partial<ViewState> = {},
): ViewState {
  return {
    size: { columns, rows },
    focus: "queue",
    selected: { agents: 0, pipeline: 0, queue: 0, findings: 0, work: 0 },
    problemsOnly: false,
    paused: false,
    link: "ok",
    linkAge: "1s",
    clock: "15:02:23",
    intervalSeconds: 2,
    nowMs: NOW,
    tick: 0,
    rings: showcaseRings(),
    highlight: new Map(),
    notice: null,
    ...overrides,
  };
}
