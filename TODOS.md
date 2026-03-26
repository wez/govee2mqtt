# TODOs

## Cache device_list_scenes() results
**What:** Make the existing flat `device_list_scenes()` read from the same catalog cache as `device_list_scenes_categorized()`.
**Why:** The existing function hits the Govee API on every call with no caching, same as the new categorized version. Once the catalog cache exists (Fix #1 in the scene-cycle PR), the flat function should read from it too.
**Pros:** Fewer API calls globally, consistent behavior, reduces rate-limit risk for all scene interactions.
**Cons:** Minor additional scope, needs cache invalidation on device re-enumeration.
**Context:** `state.rs:600` already has a TODO comment: "some plumbing to maintain offline scene controls for preferred-LAN control". This aligns with that goal.
**Depends on:** Scene Quick-Cycle PR's catalog cache implementation (Fix #1).
