// Decoy type forces the netcore metadata tables to grow by one
// TypeDef + a couple of TypeRef/MethodRef entries before `Foo`,
// shifting every later index by N. `Foo` itself is byte-identical
// to the `from` side.
namespace Demo {
    public class _Decoy {
        public string Hello() => System.Text.Encoding.UTF8.GetString(new byte[] { 104, 105 });
    }

    public class Foo {
        public int Compute(int x) {
            return x.GetHashCode() + (-x).ToString().Length;
        }
    }
}
