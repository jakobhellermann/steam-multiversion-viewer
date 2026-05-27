// Same `Demo.Foo` as the `to` side, byte-identical IL. The `to`
// side has an extra unrelated type sandwiched in front so its
// metadata-table indices shift — TypeRef/MethodRef numbers visited
// by `Foo`'s IL operands point to the same external entries but
// land at different positions. ilspy renders both Foos identically;
// dll-diff currently flags this as `Changed`, which is the false
// positive we want to eliminate.
namespace Demo {
    public class Foo {
        public int Compute(int x) {
            return x.GetHashCode() + (-x).ToString().Length;
        }
    }
}
