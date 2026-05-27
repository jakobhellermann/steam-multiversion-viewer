// Byte-identical source to from.cs. The only difference: lib.dll is
// pulled in as a `Reference`, so `Shared.Helper` is a TypeReference
// here (defined in lib.dll, not to.dll). dll-diff should treat the
// user-visible class `Demo.User` as Unchanged across the pair.
namespace Demo {
    public class User {
        public int Use(int x) {
            var h = new Shared.Helper();
            return h.Compute(x);
        }
    }
}
