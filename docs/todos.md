# TODOs

## Bugs
- Body-diff (right panel) doesn't apply pptr identity-normalization, only
  the tree status does. So a genuinely-changed node shows its pure
  pptr-renumber fields as `-`/`+` noise alongside the real change. The
  tree prunes renumber-only nodes correctly (`pptr_target_identity`), but
  `unity_serialized_node_body` dumps each side's raw JSON (markers carry
  the raw PathID in `ref`) and text-diffs them, so renumbers reappear.
   - Repro: http://localhost:6555/apps/367520/depots/367523/manifests/708613018541602983/diff?path=hollow_knight_Data%2Flevel100&target_depot_id=367523&target_manifest_id=5829533265112705522&target_branch=1.5.78.11833#mod:obj:5445,obj:5417
   - Open questions before fixing: should a renumber-only line vanish
     entirely or show identity-normalized-but-unchanged? And the jump
     link's `ref` still needs the real PathID (`#obj:5445`) — so identity
     for *comparison* and PathID for *navigation* must be separable
     (likely a separate marker field, or normalize only at diff time
     while keeping the original markers for render).
- Reconnect steam-vent after failure, idle connection, laptop suspend
   - "Also: there's an upstream bug — steam-vent's read-side logs
     `ConnectionReset` but doesn't mark the connection dead, so the writer
     keeps heartbeating into the void until the local socket end closes.
     Worth filing once we're sure."

## View formats
- Unity component-type-specific views (TextComponent, Texture2D, Shader)

## UX
- Login

## Polish
- readme, docs, screenshots

## Perf
- check for unnecessary requests, caching, profile website and backend

## TrackedChunkStore (WIP commit `yp`)
- enqueue + tracker double-count the same sha (over-count, compressed vs
  uncompressed unit mismatch in bytes counters).
- two concurrent rabex-env calls for the same sha also double-count.
- cancel mid-tracked-fetch: tracker's completion runs after stats reset
  → completed > total in drawer.


## Edge casees

- http://localhost:6555/apps/1030300/depots/1030301/manifests/4421626056705534276/diff?path=Hollow+Knight+Silksong_Data%2FStreamingAssets%2Faa%2FStandaloneWindows64%2Fatlases_assets_assets%2Fsprites%2F_atlases%2Fabyss.spriteatlas.bundle&target_depot_id=1030301&target_manifest_id=468692862190470536#archive:CAB-bf54a70ab04d641cdc3c945b2a30ad8d/obj:-7505336056661461232
Diff von file-internen pptrs.
