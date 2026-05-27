// No explicit static constructor → no `.cctor` in the metadata.
// User-visible body of `Foo` is identical to `from.cs` modulo the
// static-ctor declaration, which is the asymmetry the filter exists
// to absorb.
namespace Demo {
    public class Foo {
        public int Get() { return 42; }
    }
}
