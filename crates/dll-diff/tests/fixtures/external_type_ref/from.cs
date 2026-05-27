// `Shared.Helper` is inlined into from.dll → appears as a
// TypeDefinition in the metadata.
namespace Demo {
    public class User {
        public int Use(int x) {
            var h = new Shared.Helper();
            return h.Compute(x);
        }
    }
}
