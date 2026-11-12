import runpy
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "check-rust-module-layout.py"
CHECKER = runpy.run_path(str(SCRIPT_PATH))


class TestRustModuleLayout(unittest.TestCase):
    def test_accepts_clean_module_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            module_file = repository / "apps/desktop/src/mail.rs"
            module_file.parent.mkdir(parents=True)
            module_file.write_text("mod list;\n", encoding="utf-8")
            (module_file.parent / "mail").mkdir()
            (module_file.parent / "mail/list.rs").write_text("", encoding="utf-8")
            (repository / "crates/domain/src/lib.rs").parent.mkdir(parents=True)
            (repository / "crates/domain/src/lib.rs").write_text("", encoding="utf-8")

            violations = CHECKER["module_layout_violations"](repository)

        self.assertEqual([], violations)

    def test_rejects_path_attributes_in_apps_and_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            for source_root in ("apps", "crates"):
                source_file = repository / source_root / "example.rs"
                source_file.parent.mkdir(parents=True, exist_ok=True)
                source_file.write_text(
                    '#[path = "legacy.rs"]\nmod legacy;\n', encoding="utf-8"
                )

            violations = CHECKER["module_layout_violations"](repository)

        self.assertEqual(2, len(violations))
        self.assertIn("apps/example.rs:1: #[path] is forbidden", violations[0])
        self.assertIn("crates/example.rs:1: #[path] is forbidden", violations[1])

    def test_rejects_multiline_path_attribute(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            source_file = repository / "crates/example/src/lib.rs"
            source_file.parent.mkdir(parents=True)
            source_file.write_text(
                "#[\n    path = \"legacy.rs\"\n]\nmod legacy;\n", encoding="utf-8"
            )

            violations = CHECKER["module_layout_violations"](repository)

        self.assertEqual(1, len(violations))
        self.assertIn("crates/example/src/lib.rs:1: #[path] is forbidden", violations[0])

    def test_rejects_mod_rs(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            module_file = repository / "crates/example/src/feature/mod.rs"
            module_file.parent.mkdir(parents=True)
            module_file.write_text("", encoding="utf-8")

            violations = CHECKER["module_layout_violations"](repository)

        self.assertEqual(1, len(violations))
        self.assertIn("crates/example/src/feature/mod.rs: mod.rs is forbidden", violations[0])

    def test_ignores_rust_files_outside_apps_and_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            repository = Path(directory)
            reference_file = repository / "docs/example.rs"
            reference_file.parent.mkdir()
            reference_file.write_text(
                '#[path = "legacy.rs"]\nmod legacy;\n', encoding="utf-8"
            )

            violations = CHECKER["module_layout_violations"](repository)

        self.assertEqual([], violations)


if __name__ == "__main__":
    unittest.main()
