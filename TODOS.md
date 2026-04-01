# TODOs

## Cache device_list_scenes() results
**What:** Make the existing flat `device_list_scenes()` read from the same catalog cache as `device_list_scenes_categorized()`.
**Why:** The existing function hits the Govee API on every call with no caching, same as the new categorized version. Once the catalog cache exists (Fix #1 in the scene-cycle PR), the flat function should read from it too.
**Pros:** Fewer API calls globally, consistent behavior, reduces rate-limit risk for all scene interactions.
**Cons:** Minor additional scope, needs cache invalidation on device re-enumeration.
**Context:** `state.rs:600` already has a TODO comment: "some plumbing to maintain offline scene controls for preferred-LAN control". This aligns with that goal.
**Depends on:** Scene Quick-Cycle PR's catalog cache implementation (Fix #1).

## Enrich Platform API scenes with undoc API icons/hints
**What:** For devices that go through Platform API (which only returns scene names), try the undocumented API as a secondary source to add icon URLs and hint text.
**Why:** Platform API `EnumOption` (platform_api.rs:960) has no icon or hint fields. Undoc API has both. Enriching would give more devices the full v2 Scene Deck card experience with thumbnails and descriptions.
**Pros:** More devices get visual scene previews instead of text-only fallback.
**Cons:** ~20 lines of code, risk of name mismatches between the two APIs (Platform vs undoc may use different scene name strings for the same scene). Needs careful matching logic.
**Context:** `fetch_scene_catalog` in state.rs:670-720 tries Platform API first; if it succeeds it returns immediately without trying undoc API. A hybrid approach would try both and merge by scene name.
**Depends on:** Scene Deck v2 (hint field in SceneCatalogEntry).
