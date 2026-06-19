// TODO(ai-review): review for style and correctness
// Minimal reproduction of the `Constants.GAME_VERSION` const that older
// Unity games (e.g. Hollow Knight) keep their real shipping version in
// while `PlayerSettings.bundleVersion` stays "1.0". A `const string`
// compiles to a literal in the .NET constant table, which is what
// `extract_game_version` reads.
public static class Constants
{
    public const string GAME_VERSION = "1.5.78.11833";
}
