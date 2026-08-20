import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_gdx_rb import build_evidence, parse_markers


class VerifyGdxRbTests(unittest.TestCase):
    def test_parses_only_explicit_json_markers(self) -> None:
        output = "\n".join(
            (
                "build output that may include private details",
                'POLAR_GDX_VERIFY_SELECTED {"discovered_go_direct_count":1}',
                'POLAR_GDX_VERIFY_STATUS {"phase":"streaming"}',
                'POLAR_GDX_VERIFY_COMPLETE {"schema":"physical","result":"pass"}',
            )
        )
        parsed = parse_markers(output)
        self.assertEqual(parsed["selection"]["discovered_go_direct_count"], 1)
        self.assertEqual(parsed["status_updates"], [{"phase": "streaming"}])
        self.assertEqual(parsed["completion"]["result"], "pass")
        self.assertNotIn("private", json.dumps(parsed))

    def test_builds_failure_without_retaining_raw_output(self) -> None:
        evidence = build_evidence({}, 1, "2026-08-20T00:00:00+00:00")
        self.assertEqual(evidence["result"], "fail")
        self.assertEqual(evidence["failure"]["code"], "RUNNER_OR_BUILD_FAILED")
        self.assertFalse(evidence["identity_retained"])
        self.assertNotIn("stdout", evidence)
        self.assertNotIn("stderr", evidence)


if __name__ == "__main__":
    unittest.main()
