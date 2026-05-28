// TODO(ai-review): review for style and correctness
// Minimal reproduction of the `SecPlayerPrefs.SecurePlayerPrefs`
// shape we extract the AES key from at runtime. The static field
// initializer compiles to `ldstr <KEY>; call UTF8.GetBytes; stsfld
// keyArray` inside .cctor — the exact pattern `extract_key` matches.
using System.Text;

namespace SecPlayerPrefs
{
    public class SecurePlayerPrefs
    {
        private static byte[] keyArray = Encoding.UTF8.GetBytes("UKu52ePUBwetZ9wNX88o54dnfKRu0T1l");
    }
}
