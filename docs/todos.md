# TODOs

## Bugs
- Reconnect steam-vent after failure, idle connection, laptop suspend
   - "Also: there's an upstream bug — steam-vent's read-side logs
     `ConnectionReset` but doesn't mark the connection dead, so the writer
     keeps heartbeating into the void until the local socket end closes.
     Worth filing once we're sure."

## View formats
- Unity component-type-specific views (TextComponent, Texture2D, Shader)

## Deployment
- `rust-embed` the frontend

## UX
- Login

## Polish
- readme, docs, screenshots

## Perf
- check for unnecessary requests, caching, profile website and backend
