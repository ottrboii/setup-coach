import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import make_latest_json as m  # noqa: E402


class LatestJson(unittest.TestCase):
    def test_manifest_shape(self):
        d = m.build_manifest("v0.2.0", "v0.2.0", "ottrboii/setup-coach", "Setup Coach_0.2.0_x64-setup.exe", "SIG\n", "Hello", "2026-01-01T00:00:00Z")
        self.assertEqual(d["version"], "0.2.0")
        self.assertEqual(d["notes"], "Hello")
        p = d["platforms"]["windows-x86_64"]
        self.assertEqual(p["signature"], "SIG")
        self.assertEqual(p["url"], "https://github.com/ottrboii/setup-coach/releases/download/v0.2.0/Setup.Coach_0.2.0_x64-setup.exe")

    def test_default_notes(self):
        d = m.build_manifest("0.2.0", "v0.2.0", "o/r", "a.exe", "S", "  ")
        self.assertEqual(d["notes"], m.DEFAULT_NOTES)

    def test_rejects_bad_version_and_empty_sig(self):
        with self.assertRaises(ValueError):
            m.build_manifest("main", "v1", "o/r", "a.exe", "S")
        with self.assertRaises(ValueError):
            m.build_manifest("1.0.0", "v1.0.0", "o/r", "a.exe", " ")

    def test_cli_writes_json(self):
        with tempfile.TemporaryDirectory() as t:
            sig = os.path.join(t, "a.sig")
            out = os.path.join(t, "latest.json")
            open(sig, "w").write("abc==\n")
            self.assertEqual(m.main(["--version", "1.2.3", "--tag", "v1.2.3", "--repo", "o/r", "--installer", "a b.exe", "--sig-file", sig, "--out", out]), 0)
            d = json.load(open(out))
            self.assertEqual(d["version"], "1.2.3")
            self.assertTrue(d["platforms"]["windows-x86_64"]["url"].endswith("/a.b.exe"))


if __name__ == "__main__":
    unittest.main()
