// TODO(ai-review): review for style and correctness
import { parseSearchWith, stringifySearchWith } from "@tanstack/react-router";

/// URL search (de)serialisation shared by the app and test routers:
/// values stay strings — no JSON quoting, no number coercion. Steam
/// manifest IDs are i64s beyond `Number.MAX_SAFE_INTEGER`, so the
/// router's default JSON-ish round-trip loses precision; and quoted
/// values (`?compare_to=%22…%22`) make URLs unreadable.
export const parseSearch = parseSearchWith((search) => search);
export const stringifySearch = stringifySearchWith((value) =>
  typeof value === "string" ? value : String(value),
);
