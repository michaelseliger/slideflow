// Browser-mode fixtures. When the app runs in a plain browser (no Tauri IPC),
// `api.ts` falls back to these so `pnpm dev` gives a fully clickable app with a
// realistic-feeling library of ~40 slides across a handful of decks.

import type {
  ComposeReport,
  DeckRecord,
  DuplicateGroup,
  EmbeddingStatus,
  ExportRecord,
  ExportReport,
  FitMode,
  FontDownloadEvent,
  FontFamily,
  AddFontsResult,
  RootRecord,
  SavedSearch,
  SearchFilters,
  SearchHistoryEntry,
  SearchHit,
  SimilarSlide,
  SlideDragPaths,
  SlidePick,
  SlideRecord,
  Stats,
  StatsOverview,
  TagRecord,
} from "./types";

const EMU_W = 12_192_000;
const EMU_H = 6_858_000;

interface DeckSeed {
  title: string;
  file: string;
  folder: string;
  author: string;
  accent: string;
  /** Optional override so one seed differs in slide size — exercises the
   *  mixed-dimensions tray badge in browser mode. Defaults to EMU_W/EMU_H. */
  dims?: { w: number; h: number };
  slides: { title: string; body: string; notes?: string }[];
}

const DECK_SEEDS: DeckSeed[] = [
  {
    title: "Q3 Business Review",
    file: "Q3-Business-Review.pptx",
    folder: "/Users/you/Decks/Finance",
    author: "Jane Doe",
    accent: "#0A84FF",
    slides: [
      { title: "Q3 Business Review", body: "Fiscal year 2026 · Confidential", notes: "Welcome the exec team; set the frame for the quarter." },
      { title: "Revenue up 18% YoY", body: "ARR crossed $42M this quarter with net retention at 121%.", notes: "Emphasize durable growth, not one-time deals." },
      { title: "Pipeline Health", body: "Coverage 3.4x · Win rate 27% · Sales cycle down 9 days" },
      { title: "Churn Down to 4.1%", body: "Gross churn improved on the back of the new onboarding flow." },
      { title: "Gross Margin", body: "Blended gross margin held at 78% despite infra growth." },
      { title: "Cash Runway", body: "22 months of runway at current burn; efficient growth intact." },
    ],
  },
  {
    title: "Product Roadmap 2026",
    file: "Product-Roadmap-2026.pptx",
    folder: "/Users/you/Decks/Product",
    author: "Miguel Santos",
    accent: "#30D158",
    slides: [
      { title: "Product Roadmap 2026", body: "Search, Compose, Collaborate" },
      { title: "Ship Instant Search", body: "Sub-100ms keystroke-to-results on a local FTS5 index.", notes: "This is the headline; demo it live." },
      { title: "Compose With Fidelity", body: "Dragged slides keep original theme, master and formatting." },
      { title: "Real-time Collaboration", body: "Shared trays and comment threads land in H2." },
      { title: "Offline First", body: "Everything works without a network; sync is additive." },
      { title: "Enterprise Controls", body: "SSO, audit logs, and folder-level permissions." },
    ],
  },
  {
    title: "Brand Guidelines",
    file: "Brand-Guidelines.pptx",
    folder: "/Users/you/Decks/Marketing",
    author: "Priya Nair",
    accent: "#FF375F",
    dims: { w: 9_144_000, h: 6_858_000 },
    slides: [
      { title: "Brand Guidelines", body: "Voice, color, and typography" },
      { title: "Our Voice", body: "Warm, precise, never corporate. Speak like a helpful colleague." },
      { title: "Color System", body: "One restrained accent; neutral greys everywhere else." },
      { title: "Typography", body: "System font stack. Tabular numerals for data." },
      { title: "Logo Usage", body: "Clear space equals the height of the mark. Never re-color." },
    ],
  },
  {
    title: "All-Hands Update",
    file: "All-Hands-March.pptx",
    folder: "/Users/you/Decks/Company",
    author: "Jane Doe",
    accent: "#BF5AF2",
    slides: [
      { title: "All-Hands · March", body: "Team update and Q&A" },
      { title: "Welcome New Folks", body: "Twelve new teammates across product, sales, and support." },
      { title: "Customer Wins", body: "Three lighthouse logos went live this month." },
      { title: "What We Learned", body: "Ship smaller, measure sooner, and talk to users weekly." },
      // Deliberate EXACT duplicate of the Q3 Business Review churn slide, so
      // browser mode demos duplicate detection + the tray warning.
      { title: "Churn Down to 4.1%", body: "Gross churn improved on the back of the new onboarding flow." },
      { title: "Open Q&A", body: "Drop questions in the thread; we'll get to all of them." },
    ],
  },
  {
    title: "Sales Playbook",
    file: "Sales-Playbook.pptx",
    folder: "/Users/you/Decks/Sales",
    author: "Alex Kim",
    accent: "#FF9F0A",
    slides: [
      { title: "Sales Playbook", body: "Discovery to close" },
      { title: "Qualify With MEDDIC", body: "Metrics, economic buyer, decision criteria, and champion." },
      { title: "Handle Objections", body: "Acknowledge, reframe, and return to business value." },
      { title: "Pricing Conversation", body: "Anchor on outcomes; never lead with the discount." },
      { title: "Close and Handoff", body: "A clean handoff to onboarding sets up net retention." },
    ],
  },
  {
    title: "Design System",
    file: "Design-System.pptx",
    folder: "/Users/you/Decks/Product",
    author: "Miguel Santos",
    accent: "#64D2FF",
    slides: [
      { title: "Design System", body: "Components, tokens, and patterns" },
      { title: "8px Spacing Grid", body: "Consistent gutters keep dense grids calm and legible." },
      { title: "Elevation", body: "Hairline separators over heavy borders. Soft shadows on lift." },
      { title: "Motion", body: "Spring physics on meaningful moments only. Respect reduced motion." },
      { title: "Dark Mode", body: "Elevated greys, matted thumbnails, vibrant text on materials only." },
      // Deliberate NEAR duplicate of the Product Roadmap search slide (same
      // title, slightly reworded body) — demos the "near" duplicate badge once
      // the mock model is "downloaded".
      { title: "Ship Instant Search", body: "Sub-100ms keystroke-to-results on the local FTS5 index." },
    ],
  },
];

/** A single scan skip surfaced in browser mode so the Problems section and the
 *  live scan `skipped[]` both have something to render. Shared with the scan
 *  simulation in `api.ts`. */
export const MOCK_SCAN_ISSUE = {
  path: "/Users/you/Decks/Archive/Q1-Draft-corrupt.pptx",
  reason: "invalid zip: could not read central directory",
};

function svgFor(deck: DeckSeed, title: string, body: string): string {
  const esc = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const W = 960;
  const H = 540;
  // Wrap the body naively at ~44 chars.
  const words = body.split(" ");
  const lines: string[] = [];
  let cur = "";
  for (const w of words) {
    if ((cur + " " + w).trim().length > 44) {
      lines.push(cur.trim());
      cur = w;
    } else {
      cur += " " + w;
    }
  }
  if (cur.trim()) lines.push(cur.trim());
  const bodyTspans = lines
    .slice(0, 4)
    .map(
      (l, i) =>
        `<text x="64" y="${300 + i * 40}" font-family="-apple-system, Helvetica, Arial, sans-serif" font-size="26" fill="#3a3a3c">${esc(
          l,
        )}</text>`,
    )
    .join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}"><rect width="${W}" height="${H}" fill="#ffffff"/><rect x="0" y="0" width="14" height="${H}" fill="${deck.accent}"/><rect x="64" y="150" width="120" height="8" rx="4" fill="${deck.accent}"/><text x="64" y="120" font-family="-apple-system, Helvetica, Arial, sans-serif" font-size="46" font-weight="700" fill="#1c1c1e">${esc(
    title,
  )}</text>${bodyTspans}<text x="${W - 64}" y="${H - 40}" text-anchor="end" font-family="-apple-system, Helvetica, Arial, sans-serif" font-size="18" fill="#c7c7cc">${esc(
    deck.title,
  )}</text></svg>`;
}

let mockDecks: DeckRecord[] = [];
let mockSlides: SlideRecord[] = [];
const svgById = new Map<number, string>();

/** Tiny deterministic hash (djb2, hex) — identical title+body across decks
 *  collide, mirroring the engine's authored-content hash semantics. */
function fakeContentHash(title: string, body: string): string {
  const s = `${title} ${body}`;
  let h = 5381;
  for (let i = 0; i < s.length; i++) {
    h = ((h << 5) + h + s.charCodeAt(i)) >>> 0;
  }
  return `mock-${h.toString(16).padStart(8, "0")}`;
}

function buildMockLibrary() {
  mockDecks = [];
  mockSlides = [];
  svgById.clear();
  let deckId = 1;
  let slideId = 1;
  const now = Math.floor(Date.now() / 1000);
  for (const seed of DECK_SEEDS) {
    const deck: DeckRecord = {
      id: deckId,
      path: `${seed.folder}/${seed.file}`,
      file_name: seed.file,
      title: seed.title,
      author: seed.author,
      slide_count: seed.slides.length,
      modified_unix: now - deckId * 86400 * 3,
      size_bytes: 1_200_000 + deckId * 40_000,
      slide_width_emu: seed.dims?.w ?? EMU_W,
      slide_height_emu: seed.dims?.h ?? EMU_H,
      first_seen_unix: now - deckId * 86400 * 30,
      favorite: false,
    };
    mockDecks.push(deck);
    seed.slides.forEach((s, i) => {
      const rec: SlideRecord = {
        id: slideId,
        deck_id: deckId,
        slide_index: i + 1,
        title: s.title,
        body_text: s.body,
        notes: s.notes ?? null,
        thumb_path: null,
        favorite: false,
        content_hash: fakeContentHash(s.title, s.body),
      };
      mockSlides.push(rec);
      svgById.set(slideId, svgFor(seed, s.title, s.body));
      slideId += 1;
    });
    deckId += 1;
  }
}
buildMockLibrary();

const mockRoots: RootRecord[] = (() => {
  const byFolder = new Map<string, { decks: number; slides: number }>();
  for (const seed of DECK_SEEDS) {
    const cur = byFolder.get(seed.folder) ?? { decks: 0, slides: 0 };
    cur.decks += 1;
    cur.slides += seed.slides.length;
    byFolder.set(seed.folder, cur);
  }
  // Collapse to the two top-level roots the seeds live under.
  const roots = ["/Users/you/Decks"];
  return roots.map((path, i) => {
    let decks = 0;
    let slides = 0;
    for (const [folder, v] of byFolder) {
      if (folder.startsWith(path)) {
        decks += v.decks;
        slides += v.slides;
      }
    }
    return {
      id: i + 1,
      path,
      deck_count: decks,
      slide_count: slides,
      last_scan_unix: Math.floor(Date.now() / 1000) - 3600,
      exclude_globs: [],
    };
  });
})();

function highlight(text: string, query: string): string {
  const esc = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  if (!query.trim()) return esc(text.slice(0, 120));
  const tokens = query.trim().split(/\s+/).filter(Boolean);
  let out = esc(text);
  for (const t of tokens) {
    const re = new RegExp(
      `(${t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`,
      "ig",
    );
    out = out.replace(re, "<mark>$1</mark>");
  }
  return out;
}

// --- tags (in-memory) ------------------------------------------------------
// Assignments are keyed by slide id, which is deterministic across rebuilds, so
// they survive a mock Clear & Rebuild just as native tags survive by
// (deck_path, slide_index). `slide_count` counts only slides currently indexed.
interface MockTag {
  id: number;
  name: string;
}
let mockTags: MockTag[] = [];
let mockTagSeq = 0;
const mockSlideTags = new Map<number, Set<number>>();

function findTagByName(name: string): MockTag | undefined {
  const lower = name.toLowerCase();
  return mockTags.find((t) => t.name.toLowerCase() === lower);
}
function tagSlideCount(tagId: number): number {
  const live = new Set(mockSlides.map((s) => s.id));
  let n = 0;
  for (const [sid, ids] of mockSlideTags) if (ids.has(tagId) && live.has(sid)) n += 1;
  return n;
}
function toTagRecord(t: MockTag): TagRecord {
  return { id: t.id, name: t.name, slide_count: tagSlideCount(t.id) };
}
function byName(a: TagRecord, b: TagRecord): number {
  return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
}

export const mock = {
  listRoots: async (): Promise<RootRecord[]> => structuredClone(mockRoots),

  addRoot: async (path: string): Promise<RootRecord> => {
    const rec: RootRecord = {
      id: mockRoots.length + 1,
      path,
      deck_count: 0,
      slide_count: 0,
      last_scan_unix: null,
      exclude_globs: [],
    };
    mockRoots.push(rec);
    return rec;
  },

  removeRoot: async (rootId: number): Promise<void> => {
    const i = mockRoots.findIndex((r) => r.id === rootId);
    if (i >= 0) mockRoots.splice(i, 1);
  },

  setRootExcludes: async (
    rootId: number,
    patterns: string[],
  ): Promise<RootRecord> => {
    const r = mockRoots.find((x) => x.id === rootId);
    if (!r) throw new Error("root not found");
    r.exclude_globs = patterns.map((p) => p.trim()).filter(Boolean);
    return structuredClone(r);
  },

  getDecks: async (): Promise<DeckRecord[]> => structuredClone(mockDecks),

  getDeckSlides: async (deckId: number): Promise<SlideRecord[]> =>
    structuredClone(mockSlides.filter((s) => s.deck_id === deckId)),

  getStats: async (): Promise<Stats> => ({
    deck_count: mockDecks.length,
    slide_count: mockSlides.length,
  }),

  getSlideSvg: async (slideId: number): Promise<string> =>
    svgById.get(slideId) ??
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 540"><rect width="960" height="540" fill="#eee"/></svg>`,

  // Deterministic sample drops so the "Approximate" badge is visible in `pnpm dev`.
  getSlideDropped: (slideId: number): string[] =>
    slideId === 3
      ? ["chart"]
      : slideId === 15
        ? ["smartart"]
        : slideId === 22
          ? ["ole", "unsupported-image"]
          : [],

  search: async (
    query: string,
    filters: {
      path_prefix?: string | null;
      deck_query?: string | null;
      favorites_only?: boolean | null;
      tag_id?: number | null;
      search_mode?: string | null;
    } = {},
  ): Promise<SearchHit[]> => {
    const q = query.trim().toLowerCase();
    // Semantic/hybrid only kick in with the mock model "ready" (like native,
    // which silently degrades to lexical otherwise).
    const mode = filters.search_mode ?? "lexical";
    const semantic =
      q.length > 0 &&
      (mode === "semantic" || mode === "hybrid") &&
      mockSemanticEnabled &&
      mockModelDownloaded;
    const qTokens = q.split(/\s+/).filter(Boolean);
    const hits: SearchHit[] = [];
    for (const slide of mockSlides) {
      const deck = mockDecks.find((d) => d.id === slide.deck_id)!;
      if (filters.path_prefix && !deck.path.startsWith(filters.path_prefix)) {
        continue;
      }
      if (filters.favorites_only && !slide.favorite) continue;
      if (filters.tag_id != null && !mockSlideTags.get(slide.id)?.has(filters.tag_id)) {
        continue;
      }
      if (
        filters.deck_query &&
        !`${deck.title} ${deck.file_name}`
          .toLowerCase()
          .includes(filters.deck_query.toLowerCase())
      ) {
        continue;
      }
      const hay = `${slide.title ?? ""} ${slide.body_text} ${
        slide.notes ?? ""
      }`.toLowerCase();
      const lexicalMatch = !q || hay.includes(q);
      // Fake "semantic" recall: any shared word counts, so reworded slides
      // surface without an exact substring (demoing semantic-only hits).
      const tokenOverlap = semantic
        ? qTokens.filter((t) => hay.includes(t)).length
        : 0;
      if (!lexicalMatch && !(semantic && tokenOverlap > 0)) continue;
      if (mode === "semantic" && !semantic && !lexicalMatch) continue;
      const source = slide.body_text.toLowerCase().includes(q)
        ? slide.body_text
        : slide.title ?? slide.body_text;
      hits.push({
        slide: structuredClone(slide),
        deck: structuredClone(deck),
        // Semantic-only hits carry a plain (mark-free) snippet, like native.
        snippet: lexicalMatch
          ? highlight(source, query)
          : source.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").slice(0, 160),
        score: q
          ? (lexicalMatch ? (slide.title?.toLowerCase().includes(q) ? 2 : 1) : 0) +
            tokenOverlap * 0.3
          : 0,
      });
    }
    hits.sort((a, b) => b.score - a.score || a.slide.id - b.slide.id);
    return hits;
  },

  composeDeck: async (
    picks: SlidePick[],
    outputPath: string,
    title: string,
    _includeNotes: boolean,
    _fitMode?: FitMode,
  ): Promise<ComposeReport> => {
    // Simulate assembly latency so the progress UI is exercised in the browser.
    await new Promise((r) => setTimeout(r, 700));
    const decks = new Set(picks.map((p) => p.pptx_path));
    mockExports.unshift({
      output_path: outputPath,
      title,
      slide_count: picks.length,
      source_decks: decks.size,
      exported_unix: Math.floor(Date.now() / 1000),
    });
    for (const p of picks) mockExportCounts[p.pptx_path] = (mockExportCounts[p.pptx_path] ?? 0) + 1;
    return {
      output_path: outputPath,
      slides_written: picks.length,
      source_decks: decks.size,
      warnings: [],
      notes: [],
    };
  },

  // PNG/PDF export (WS-D). `kind` is a PNG width, or "pdf" for a single PDF.
  // Bumps the same "Most exported" counters as composeDeck so the stats view
  // reacts in browser mode too.
  exportTray: async (
    picks: SlidePick[],
    target: string,
    kind: number | "pdf",
  ): Promise<ExportReport> => {
    await new Promise((r) => setTimeout(r, 200));
    const decks = new Set(picks.map((p) => p.pptx_path));
    const isPdf = kind === "pdf";
    const files_written = isPdf
      ? [target]
      : picks.map((p, i) => {
          const stem = (p.pptx_path.split("/").pop() ?? "deck").replace(/\.pptx$/i, "");
          return `${target}/${String(i + 1).padStart(3, "0")} — ${stem} — slide ${p.slide_index}.png`;
        });
    mockExports.unshift({
      output_path: target,
      title: (target.split("/").pop() ?? "Export").replace(/\.[^.]+$/, ""),
      slide_count: isPdf ? picks.length : files_written.length,
      source_decks: decks.size,
      exported_unix: Math.floor(Date.now() / 1000),
    });
    for (const p of picks)
      mockExportCounts[p.pptx_path] = (mockExportCounts[p.pptx_path] ?? 0) + 1;
    return { files_written, warnings: [] };
  },

  // --- native drag-out (WS-G) ----------------------------------------------
  // Native-only feature: the browser UI hides it (gated on isTauri), so these
  // just log and no-op. Kept for lib/mock.ts parity with lib/api.ts.
  prepareSlideDrag: async (pick: SlidePick): Promise<SlideDragPaths> => {
    console.info("[mock] prepare slide drag:", pick.pptx_path, pick.slide_index);
    return { pptx: "", icon: "" };
  },

  startNativeDrag: async (paths: string[], icon: string): Promise<void> => {
    console.info("[mock] start native drag:", paths, icon);
  },

  // --- favorites / stats ---------------------------------------------------

  toggleFavoriteSlide: async (slideId: number): Promise<boolean> => {
    const slide = mockSlides.find((s) => s.id === slideId);
    if (!slide) return false;
    slide.favorite = !slide.favorite;
    return slide.favorite;
  },

  toggleFavoriteDeck: async (deckId: number): Promise<boolean> => {
    const deck = mockDecks.find((d) => d.id === deckId);
    if (!deck) return false;
    deck.favorite = !deck.favorite;
    return deck.favorite;
  },

  // --- tags ----------------------------------------------------------------

  listTags: async (): Promise<TagRecord[]> => mockTags.map(toTagRecord).sort(byName),

  getSlideTags: async (slideId: number): Promise<TagRecord[]> => {
    const ids = mockSlideTags.get(slideId) ?? new Set<number>();
    return mockTags
      .filter((t) => ids.has(t.id))
      .map(toTagRecord)
      .sort(byName);
  },

  setSlideTags: async (slideId: number, names: string[]): Promise<void> => {
    const desired: number[] = [];
    for (const raw of names) {
      const name = raw.trim();
      if (!name) continue;
      let tag = findTagByName(name);
      if (!tag) {
        mockTagSeq += 1;
        tag = { id: mockTagSeq, name };
        mockTags.push(tag);
      }
      if (!desired.includes(tag.id)) desired.push(tag.id);
    }
    if (desired.length === 0) mockSlideTags.delete(slideId);
    else mockSlideTags.set(slideId, new Set(desired));
    // Prune tags with no remaining assignments anywhere.
    const assigned = new Set<number>();
    for (const ids of mockSlideTags.values()) for (const id of ids) assigned.add(id);
    mockTags = mockTags.filter((t) => assigned.has(t.id));
  },

  renameTag: async (tagId: number, name: string): Promise<void> => {
    const trimmed = name.trim();
    if (!trimmed) throw new Error("Tag name cannot be empty");
    const clash = mockTags.find(
      (t) => t.id !== tagId && t.name.toLowerCase() === trimmed.toLowerCase(),
    );
    if (clash) throw new Error(`A tag named “${trimmed}” already exists`);
    const t = mockTags.find((x) => x.id === tagId);
    if (t) t.name = trimmed;
  },

  deleteTag: async (tagId: number): Promise<void> => {
    mockTags = mockTags.filter((t) => t.id !== tagId);
    for (const ids of mockSlideTags.values()) ids.delete(tagId);
  },

  // --- clear / rebuild -----------------------------------------------------

  // Mirror the native clear_index: wipe indexed content + history but keep
  // roots. Browser mode has no separate favorites store, so mock favorites
  // reset with the records here — acceptable (native keeps them via the DB).
  clearIndex: async (): Promise<void> => {
    mockDecks = [];
    mockSlides = [];
    svgById.clear();
    mockSearches.length = 0;
    mockExports.length = 0;
    // Native clear() deletes export_history, cascading export_picks, so
    // get_export_counts() returns empty afterwards. Mirror that here so the
    // "Most exported" sort doesn't keep ranking by pre-clear seeded counts.
    for (const k of Object.keys(mockExportCounts)) delete mockExportCounts[k];
  },

  // Re-read the "folders" on rescan: deterministic ids make this idempotent.
  rebuildFromDisk: async (): Promise<void> => {
    buildMockLibrary();
  },

  recordSearch: async (query: string, resultCount: number): Promise<void> => {
    const q = query.trim();
    if (!q) return;
    const last = mockSearches[0];
    if (last && (q.startsWith(last.query) || last.query.startsWith(q))) {
      last.query = q;
      last.result_count = resultCount;
      last.searched_unix = Math.floor(Date.now() / 1000);
      return;
    }
    mockSearches.unshift({
      query: q,
      result_count: resultCount,
      searched_unix: Math.floor(Date.now() / 1000),
    });
  },

  getStatsOverview: async (): Promise<StatsOverview> => ({
    deck_count: mockDecks.length,
    slide_count: mockSlides.length,
    total_bytes: mockDecks.reduce((n, d) => n + d.size_bytes, 0),
    favorite_slides: mockSlides.filter((s) => s.favorite).length,
    favorite_decks: mockDecks.filter((d) => d.favorite).length,
    last_scan: {
      started_unix: Math.floor(Date.now() / 1000) - 3600,
      duration_ms: 1840,
      indexed: mockDecks.length,
      removed: 0,
      unchanged: 0,
      skipped: 1,
    },
    recent_searches: structuredClone(mockSearches.slice(0, 10)),
    recent_exports: structuredClone(mockExports.slice(0, 10)),
    largest_decks: structuredClone(
      [...mockDecks].sort((a, b) => b.size_bytes - a.size_bytes).slice(0, 5),
    ),
    last_scan_issues: [MOCK_SCAN_ISSUE],
    render_drops: [
      { kind: "chart", slides: 2 },
      { kind: "smartart", slides: 1 },
    ],
  }),

  getExportCounts: async (): Promise<Record<string, number>> => ({ ...mockExportCounts }),

  // --- saved searches ------------------------------------------------------

  listSavedSearches: async (): Promise<SavedSearch[]> => structuredClone(mockSavedSearches),

  saveSearch: async (
    name: string,
    query: string,
    filters: SearchFilters,
  ): Promise<SavedSearch> => {
    const rec: SavedSearch = {
      id: mockSavedId++,
      name,
      query,
      filters: structuredClone(filters),
      created_unix: Math.floor(Date.now() / 1000),
    };
    mockSavedSearches.push(rec);
    return structuredClone(rec);
  },

  renameSavedSearch: async (id: number, name: string): Promise<void> => {
    const s = mockSavedSearches.find((x) => x.id === id);
    if (s) s.name = name;
  },

  deleteSavedSearch: async (id: number): Promise<void> => {
    const i = mockSavedSearches.findIndex((x) => x.id === id);
    if (i >= 0) mockSavedSearches.splice(i, 1);
  },

  setAutoUpdateEnabled: async (enabled: boolean): Promise<void> => {
    mockAutoUpdate = enabled;
  },

  // Mirror the native get_auto_update_enabled read-back so the Settings toggle
  // can reconcile against the backend's source of truth in browser mode too.
  getAutoUpdateEnabled: async (): Promise<boolean> => mockAutoUpdate,

  // --- semantic search -------------------------------------------------------

  getEmbeddingStatus: async (): Promise<EmbeddingStatus> => ({
    state: mockModelDownloading
      ? "downloading"
      : !mockSemanticEnabled
        ? "disabled"
        : !mockModelDownloaded
          ? "not_downloaded"
          : "ready",
    model_id: "intfloat/multilingual-e5-small",
    dims: 384,
    embedded_slides: mockEmbeddedSlides,
    total_slides: mockSlides.length,
    error: null,
  }),

  setSemanticSearchEnabled: async (enabled: boolean): Promise<void> => {
    mockSemanticEnabled = enabled;
  },

  deleteEmbeddingModel: async (): Promise<void> => {
    mockModelDownloaded = false;
    mockSemanticEnabled = false;
    mockEmbeddedSlides = 0;
  },

  // Hooks for the fake download/backfill drivers in api.ts.
  setModelDownloading: (downloading: boolean): void => {
    mockModelDownloading = downloading;
  },
  setModelDownloaded: (downloaded: boolean): void => {
    mockModelDownloaded = downloaded;
  },
  setAllEmbedded: (): void => {
    mockEmbeddedSlides = mockSlides.length;
  },

  // --- fonts -------------------------------------------------------------

  listLibraryFonts: async (): Promise<FontFamily[]> => structuredClone(mockFonts),

  fontsDir: async (): Promise<string> =>
    "/Users/you/Library/Application Support/com.slideflow.app/fonts",

  addUserFonts: async (paths: string[]): Promise<AddFontsResult> => {
    let added = 0;
    const errors: string[] = [];
    for (const p of paths) {
      if (!/\.(ttf|otf)$/i.test(p)) {
        errors.push(`${p}: not a .ttf/.otf file`);
        continue;
      }
      const base = (p.split("/").pop() ?? "Font.ttf").replace(/\.(ttf|otf)$/i, "");
      const family = base.replace(/[-_].*$/, "");
      const existing = mockFonts.find((f) => f.family.toLowerCase() === family.toLowerCase());
      if (existing) {
        existing.status = "available";
        existing.source = "user";
        existing.removable = true;
        existing.download_source = null;
      } else {
        mockFonts.push({
          family,
          status: "available",
          source: "user",
          embedded: false,
          removable: true,
          download_source: null,
        });
      }
      added += 1;
    }
    if (added > 0) emitMockFontsChanged();
    return { added, errors, fonts: structuredClone(mockFonts) };
  },

  removeAppFont: async (family: string): Promise<FontFamily[]> => {
    const row = mockFonts.find((f) => f.family === family && f.removable);
    if (row) {
      // Revert to the pre-install state: curated families become downloadable
      // again, everything else falls back to missing (or drops if it was an
      // extra font no deck names — here we keep named ones).
      const curated = MOCK_DOWNLOADABLE[family];
      row.removable = false;
      row.source = "";
      row.status = curated ? "downloadable" : "missing";
      row.download_source = curated ?? null;
    }
    emitMockFontsChanged();
    return structuredClone(mockFonts);
  },

  downloadFont: async (family: string): Promise<boolean> => {
    mockFontDownloadCanceled = false;
    emitMockFontDownload({ kind: "started", family });
    await mockSleep(700);
    if (mockFontDownloadCanceled) {
      emitMockFontDownload({ kind: "canceled", family });
      return true;
    }
    const row = mockFonts.find((f) => f.family === family);
    if (row) {
      row.status = "available";
      row.source = "downloaded";
      row.removable = true;
      row.download_source = null;
    }
    emitMockFontDownload({ kind: "done", family });
    emitMockFontsChanged();
    return true;
  },

  cancelFontDownload: async (): Promise<void> => {
    mockFontDownloadCanceled = true;
  },

  onFontDownloadEvent: (handler: (ev: FontDownloadEvent) => void): (() => void) => {
    mockFontDownloadListeners.add(handler);
    return () => mockFontDownloadListeners.delete(handler);
  },

  onFontsChanged: (handler: () => void): (() => void) => {
    mockFontsChangedListeners.add(handler);
    return () => mockFontsChangedListeners.delete(handler);
  },

  getSimilarSlides: async (slideId: number, limit: number): Promise<SimilarSlide[]> => {
    if (!mockSemanticEnabled || !mockModelDownloaded) return [];
    const anchor = mockSlides.find((s) => s.id === slideId);
    if (!anchor) return [];
    const anchorWords = new Set(
      `${anchor.title ?? ""} ${anchor.body_text}`.toLowerCase().split(/\W+/).filter(Boolean),
    );
    const scored = mockSlides
      // Exclude the anchor and exact-content twins, like native.
      .filter((s) => s.id !== slideId && s.content_hash !== anchor.content_hash)
      .map((s) => {
        const words = new Set(
          `${s.title ?? ""} ${s.body_text}`.toLowerCase().split(/\W+/).filter(Boolean),
        );
        let overlap = 0;
        for (const w of anchorWords) if (words.has(w)) overlap += 1;
        const union = anchorWords.size + words.size - overlap;
        return { slide: s, score: union > 0 ? overlap / union : 0 };
      })
      .sort((a, b) => b.score - a.score || a.slide.id - b.slide.id)
      .slice(0, limit);
    return scored.map(({ slide, score }) => ({
      slide: structuredClone(slide),
      deck: structuredClone(mockDecks.find((d) => d.id === slide.deck_id)!),
      // Map Jaccard (0..1) into a plausible cosine range for the UI.
      score: 0.55 + score * 0.44,
    }));
  },

  listDuplicateGroups: async (): Promise<DuplicateGroup[]> => {
    const groups: DuplicateGroup[] = [];
    const newestFirst = (a: { deck: DeckRecord }, b: { deck: DeckRecord }) =>
      b.deck.modified_unix - a.deck.modified_unix;
    const withDeck = (s: SlideRecord) => ({
      slide: structuredClone(s),
      deck: structuredClone(mockDecks.find((d) => d.id === s.deck_id)!),
    });
    // Exact groups: identical content_hash (always available — no model needed).
    const byHash = new Map<string, SlideRecord[]>();
    for (const s of mockSlides) {
      if (!s.content_hash) continue;
      const list = byHash.get(s.content_hash) ?? [];
      list.push(s);
      byHash.set(s.content_hash, list);
    }
    for (const list of byHash.values()) {
      if (list.length < 2) continue;
      groups.push({ kind: "exact", score: null, slides: list.map(withDeck).sort(newestFirst) });
    }
    // Near groups (model required): same title, different content (the seeded
    // reworded pair).
    if (mockSemanticEnabled && mockModelDownloaded) {
      const byTitle = new Map<string, SlideRecord[]>();
      for (const s of mockSlides) {
        if (!s.title) continue;
        const list = byTitle.get(s.title) ?? [];
        list.push(s);
        byTitle.set(s.title, list);
      }
      for (const list of byTitle.values()) {
        const distinct = new Set(list.map((s) => s.content_hash));
        if (list.length < 2 || distinct.size < 2) continue;
        groups.push({ kind: "near", score: 0.94, slides: list.map(withDeck).sort(newestFirst) });
      }
    }
    groups.sort((a, b) => b.slides.length - a.slides.length);
    return groups;
  },
};

let mockAutoUpdate = true;
let mockSemanticEnabled = false;
let mockModelDownloaded = false;
let mockModelDownloading = false;
let mockEmbeddedSlides = 0;

const mockSleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// The source labels the curated resolver reports for its downloadable families;
// also used to revert a removed download back to "downloadable".
const MOCK_DOWNLOADABLE: Record<string, string> = {
  Karla: "Google Fonts (OFL) · github.com/google/fonts",
  Aptos: "Microsoft (free download) · microsoft.com/download id=106087",
};

// A plausible font inventory exercising every status/source the panel renders.
let mockFonts: FontFamily[] = [
  { family: "Aptos", status: "downloadable", source: "", embedded: false, removable: false, download_source: MOCK_DOWNLOADABLE.Aptos },
  { family: "Arial", status: "available", source: "system", embedded: false, removable: false, download_source: null },
  { family: "Calibri", status: "available", source: "bundled", embedded: false, removable: false, download_source: null },
  { family: "Grafton", status: "available", source: "harvested", embedded: true, removable: true, download_source: null },
  { family: "Karla", status: "downloadable", source: "", embedded: false, removable: false, download_source: MOCK_DOWNLOADABLE.Karla },
  { family: "VilleroyBoch", status: "missing", source: "", embedded: false, removable: false, download_source: null },
];
let mockFontDownloadCanceled = false;
const mockFontDownloadListeners = new Set<(ev: FontDownloadEvent) => void>();
const mockFontsChangedListeners = new Set<() => void>();
function emitMockFontDownload(ev: FontDownloadEvent) {
  for (const l of mockFontDownloadListeners) l(ev);
}
function emitMockFontsChanged() {
  for (const l of mockFontsChangedListeners) l();
}

const mockSearches: SearchHistoryEntry[] = [];
const mockExports: ExportRecord[] = [];

// Saved searches survive clearIndex() (like favorites), mirroring the native
// clear() which whitelists the tables it wipes and leaves saved_searches alone.
let mockSavedId = 1;
const mockSavedSearches: SavedSearch[] = [];

// Seed plausible export counts so "Most exported" differentiates before any
// compose in browser mode (the real backend starts empty for existing users).
const mockExportCounts: Record<string, number> = {};
mockExportCounts[`${DECK_SEEDS[0].folder}/${DECK_SEEDS[0].file}`] = 9;
mockExportCounts[`${DECK_SEEDS[1].folder}/${DECK_SEEDS[1].file}`] = 5;
mockExportCounts[`${DECK_SEEDS[4].folder}/${DECK_SEEDS[4].file}`] = 2;
