// Shared library type. On the `from` side this file is included via
// `Compile` so the type lands directly in from.dll; on the `to` side
// the file is compiled into a separate lib.dll which to.dll then
// `Reference`s.
namespace Shared {
    public class Helper {
        public int Compute(int x) => x + 1;
    }
}
