// Standalone unit tests for the pure tray data model.
//
// The desktop app has no JS test runner wired up (only `tsc --noEmit` + vite).
// Rather than pull in a framework, these tests are plain assertions over the
// pure reducers in `trayModel.ts` and are runnable directly with Node 22+:
//
//     node --experimental-strip-types src/lib/trayModel.test.ts
//
// The file typechecks as part of `tsc --noEmit` (it lives under `src`) and is
// never imported by app code, so vite tree-shakes it out of the bundle.

import type { DeckRecord, SlideRecord } from "./types";
import {
  activeItems,
  autoTrayName,
  commitItems,
  createTray,
  deleteTray,
  emptyModel,
  migrate,
  renameTray,
  switchTray,
  toPersisted,
  uidFor,
  undo,
  type TrayItem,
} from "./trayModel.ts";

let failures = 0;
function check(cond: boolean, msg: string): void {
  if (cond) {
    console.log("ok  -", msg);
  } else {
    failures += 1;
    console.error("FAIL-", msg);
  }
}
function eq<T>(a: T, b: T, msg: string): void {
  const sa = JSON.stringify(a);
  const sb = JSON.stringify(b);
  check(sa === sb, `${msg}${sa === sb ? "" : ` (got ${sa}, want ${sb})`}`);
}

// --- fixtures ---------------------------------------------------------------

function deck(id: number): DeckRecord {
  return {
    id,
    path: `/decks/d${id}.pptx`,
    file_name: `d${id}.pptx`,
    title: `Deck ${id}`,
    author: null,
    slide_count: 8,
    modified_unix: 0,
    size_bytes: 0,
    slide_width_emu: 12_192_000,
    slide_height_emu: 6_858_000,
    first_seen_unix: 0,
    favorite: false,
  };
}
function slide(deckId: number, idx: number): SlideRecord {
  return {
    id: deckId * 100 + idx,
    deck_id: deckId,
    slide_index: idx,
    title: null,
    body_text: "",
    notes: null,
    thumb_path: null,
    favorite: false,
  };
}
function item(deckId: number, idx: number): TrayItem {
  const s = slide(deckId, idx);
  return { uid: uidFor(s), slide: s, deck: deck(deckId) };
}

let idc = 0;
function freshId(): string {
  idc += 1;
  return `id-${idc}`;
}

// --- 1. v1 -> v2 localStorage migration ------------------------------------

(function v1Migration() {
  const v1Items = [item(1, 1), item(1, 2), item(2, 3)];
  const v1Raw = JSON.stringify(v1Items);
  const m = migrate(v1Raw, null, freshId);

  check(m.order.length === 1, "v1 migrate -> exactly one tray");
  const only = m.trays[m.activeId];
  check(!!only && m.activeId === m.order[0], "v1 migrate -> active tray is the sole tray");
  check(only.name === "Tray 1", "v1 migrate -> tray named 'Tray 1'");
  eq(only.items, v1Items, "v1 migrate -> items preserved identically");
  check(only.past.length === 0 && only.future.length === 0, "v1 migrate -> empty history");
  check(m.collapsed === false, "v1 migrate -> not collapsed");
})();

(function v2WinsOverV1() {
  const v1Raw = JSON.stringify([item(9, 9)]);
  const seed = createTray(emptyModel("A"), "B", "Team Deck");
  const v2Raw = JSON.stringify(toPersisted(seed));
  const m = migrate(v1Raw, v2Raw, freshId);
  eq(m.order, ["A", "B"], "v2 present -> v1 ignored, v2 order restored");
  check(m.activeId === "B", "v2 present -> active restored");
  check(m.trays["B"].name === "Team Deck", "v2 present -> names restored");
})();

(function emptyStart() {
  const m = migrate(null, null, freshId);
  check(m.order.length === 1 && m.trays[m.activeId].name === "Tray 1", "no storage -> fresh 'Tray 1'");
  eq(activeItems(m), [], "no storage -> empty active tray");
})();

// --- 2. per-tray undo isolation --------------------------------------------

(function undoIsolation() {
  let m = emptyModel("A"); // active A ("Tray 1")
  m = createTray(m, "B"); // active B ("Tray 2")
  check(m.trays["B"].name === "Tray 2", "auto-name second tray -> 'Tray 2'");

  // Populate B, then switch to A and mutate A twice.
  m = commitItems(m, "B", [item(2, 1)]);
  m = switchTray(m, "A");
  m = commitItems(m, "A", [item(1, 1)]);
  m = commitItems(m, "A", [item(1, 1), item(1, 2)]);

  // Undo on A must not touch B at all.
  m = undo(m, "A");
  eq(activeItems(m), [item(1, 1)], "undo A -> A rolled back one step");
  eq(m.trays["B"].items, [item(2, 1)], "undo A -> B items untouched");
  check(m.trays["B"].past.length === 1, "undo A -> B keeps its own history");
  check(m.trays["B"].future.length === 0, "undo A -> B redo stack untouched");
  check(m.trays["A"].future.length === 1, "undo A -> A gains one redo entry");
})();

// --- 3. delete-active-tray behaviour ---------------------------------------

(function deleteActiveMiddle() {
  let m = emptyModel("A");
  m = createTray(m, "B");
  m = createTray(m, "C");
  m = switchTray(m, "B"); // active is the middle tray
  m = deleteTray(m, "B", freshId);
  eq(m.order, ["A", "C"], "delete active middle -> order compacted");
  check(m.activeId === "C", "delete active middle -> switches to neighbour");
  check(!m.trays["B"], "delete active middle -> tray gone");
})();

(function deleteActiveLastInOrder() {
  let m = emptyModel("A");
  m = createTray(m, "B"); // active B, order [A,B]
  m = deleteTray(m, "B", freshId);
  eq(m.order, ["A"], "delete active tail -> order compacted");
  check(m.activeId === "A", "delete active tail -> falls back to previous");
})();

(function deleteLastResets() {
  const m = deleteTray(emptyModel("X"), "X", () => "NEW");
  eq(m.order, ["NEW"], "delete last -> resets to a single tray");
  check(m.trays["NEW"].name === "Tray 1", "delete last -> new tray named 'Tray 1'");
  check(m.activeId === "NEW", "delete last -> new tray active");
  eq(activeItems(m), [], "delete last -> new tray empty");
})();

(function deleteNonActiveKeepsActive() {
  let m = emptyModel("A");
  m = createTray(m, "B");
  m = createTray(m, "C"); // active C
  m = deleteTray(m, "A", freshId);
  eq(m.order, ["B", "C"], "delete non-active -> order compacted");
  check(m.activeId === "C", "delete non-active -> active unchanged");
})();

// --- extras -----------------------------------------------------------------

(function renameAndAutoName() {
  let m = emptyModel("A");
  m = renameTray(m, "A", "Pitch");
  check(m.trays["A"].name === "Pitch", "rename tray -> name updated");
  m = createTray(m, freshId()); // order.length now 2 -> next is "Tray 3"
  check(autoTrayName(m) === "Tray 3", "autoTrayName -> next unused index");
})();

(function persistRoundTrip() {
  let m = emptyModel("A");
  m = commitItems(m, "A", [item(1, 1)]);
  m = createTray(m, "B", "Backup");
  m = commitItems(m, "B", [item(2, 2)]);
  const restored = migrate(null, JSON.stringify(toPersisted(m)), freshId);
  eq(restored.order, ["A", "B"], "round-trip -> order preserved");
  check(restored.activeId === "B", "round-trip -> active preserved");
  eq(restored.trays["A"].items, [item(1, 1)], "round-trip -> tray A items preserved");
  eq(restored.trays["B"].items, [item(2, 2)], "round-trip -> tray B items preserved");
  check(
    restored.trays["A"].past.length === 0 && restored.trays["B"].future.length === 0,
    "round-trip -> history not persisted",
  );
})();

if (failures > 0) {
  throw new Error(`${failures} tray-model test(s) failed`);
}
console.log(`\nAll tray-model tests passed.`);
