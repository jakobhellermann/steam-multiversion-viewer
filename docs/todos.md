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

## DLL decompile: missing assembly references
- ilspycmd decompiles `Assembly-CSharp.dll` with no access to its sibling
  assemblies, so it can't tell value-types from reference-types for
  external types (UnityEngine.*) and litters method bodies with
  `//IL_xxxx: Unknown result type (might be due to invalid IL or missing
  references)` and `//IL_xxxx: Expected O, but got Unknown` comments.
   - Root cause: `tempfile_for` (`crates/transform/src/cache.rs`) drops the
     DLL alone into a flat `$TMPDIR/smv-transform-*` file. ILSpy's resolver
     searches the input's own directory for references and finds none.
     A clean local decomp works only because the DLL sat in `Managed/`
     next to all the `UnityEngine.*.dll`. Affects BOTH the per-type (`-t`)
     and warmer (`-p`) paths — same isolated tempfile.
   - Fix needs the sibling DLLs on disk next to the input (or via
     `ilspycmd -r <dir>`). At the decompile call sites (`structured.rs`,
     `diff.rs`) `snapshot.manifest().files` lets us enumerate every `.dll`
     in the same Managed dir and `snapshot.read_full` them — but the
     `transform` crate only gets `dll_bytes`, so the dir would have to be
     plumbed through.
   - Open: ilspycmd needs the references as real files on disk. Disliked
     options: materialize a persistent ref-dir per sibling-set under
     store_root (storage cost), or build/delete an ephemeral tempdir per
     call (constant write/delete churn). Ideal would be lazy in-memory
     references, but ilspycmd can't consume those, and FUSE isn't
     cross-platform. No good approach yet — left unimplemented.
   - Note: existing `transforms/<sha>/` cache entries were produced
     without references and won't be regenerated when this lands (cache
     key is the DLL sha only); they'd need clearing or an artifact-version
     bump to pick up cleaner output.

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
