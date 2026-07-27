// TODO(ai-review): review for style and correctness
import { http, HttpResponse } from "msw";
import type { OwnedGame } from "#/api";

export const libraryFixture: OwnedGame[] = [
  { appid: 220, name: "Half-Life 2", playtime_minutes: 720 },
  { appid: 440, name: "Team Fortress 2", playtime_minutes: 60 },
  { appid: 570, name: "Dota 2", playtime_minutes: 12 },
];

export const defaultHandlers = [
  http.get("/api/library", () => HttpResponse.json(libraryFixture)),
  http.get("/api/mount/status", () => HttpResponse.json({ mounted: false })),
  http.get("/api/auth/status", () => HttpResponse.json({ authenticated: true })),
];
