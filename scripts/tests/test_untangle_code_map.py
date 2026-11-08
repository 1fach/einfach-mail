import runpy
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "untangle-code-map.py"
CODE_MAP = runpy.run_path(str(SCRIPT_PATH))


class TestAttributeCounts(unittest.TestCase):
    def test_counts_test_paths_and_ignores_other_attributes(self):
        source_lines = [
            "#[test]",
            "#[tokio::test]",
            "#[gpui_kit::test]",
            "#[suite::nested::test(case = 1)]",
            "#[  gpui_kit :: test ]",
            "#[test_helper]",
            "#[tokio::test_attribute]",
            "#[cfg(test)]",
            "#![test]",
        ]

        self.assertEqual(5, CODE_MAP["count_test_attributes"](source_lines))

    def test_inventory_counts_are_stable(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            source_path = Path(directory) / "sample.rs"
            source_path.write_text(
                "#[gpui_kit::test]\nfn renders() {}\n#[component]\nstruct Widget;\n",
                encoding="utf-8",
            )

            first_inventory = CODE_MAP["source_inventory"]([source_path])
            second_inventory = CODE_MAP["source_inventory"]([source_path])

        self.assertEqual(first_inventory, second_inventory)
        self.assertEqual(4, first_inventory[0]["lines"])
        self.assertEqual(2, first_inventory[0]["items"])
        self.assertEqual(1, first_inventory[0]["tests"])


if __name__ == "__main__":
    unittest.main()
