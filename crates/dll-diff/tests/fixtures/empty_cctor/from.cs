// Explicit empty static constructor → the compiler emits a `.cctor`
// whose body is just `Return`. The newer build on the `to` side
// elides it entirely. dll-diff filters empty `.cctor`s from the hash
// so this purely-optimisation-driven flip doesn't register as
// Changed.
namespace Demo {
    public class Foo {
        static Foo() { }
        public int Get() { return 42; }
    }
}
