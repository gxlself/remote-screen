import re
import unittest
from pathlib import Path


HOST_DIR = Path(__file__).parents[1] / "host"
FORBIDDEN = ("Write-Host", "Add-Content", "Out-File", "Get-Counter", "Invoke-Expression")
COMMON_FUNCTIONS = (
    "Get-BenchDir",
    "Write-Log",
    "Write-Result",
    "Read-BenchParams",
    "Assert-UnderBenchDir",
    "Test-Sha256",
    "Invoke-Msi",
    "Invoke-MsiUninstall",
    "Get-UninstallEntry",
    "Start-Detached",
    "Start-Interactive",
    "Stop-Interactive",
    "Wait-Until",
    "Get-ServiceState",
    "Set-ServiceRunning",
    "Find-Edge",
    "Get-ConsoleSession",
    "Get-ProcessByCommandLine",
)


class PowerShellStaticTests(unittest.TestCase):
    def test_scripts_are_ascii_without_forbidden_commands(self):
        scripts = sorted(HOST_DIR.rglob("*.ps1"))
        self.assertTrue(scripts)
        for path in scripts:
            data = path.read_bytes()
            self.assertFalse(data.startswith(b"\xef\xbb\xbf"), path)
            self.assertLess(max(data, default=0), 0x80, path)
            text = data.decode("ascii")
            for token in FORBIDDEN:
                self.assertNotIn(token, text, f"{token} in {path}")

    def test_bench_header(self):
        text = (HOST_DIR / "bench.ps1").read_text(encoding="ascii")
        self.assertIn("$ProgressPreference = 'SilentlyContinue'", text)
        self.assertIn("UTF8Encoding($false)", text)
        self.assertRegex(text, r"\.\s*\(Join-Path\s+\$PSScriptRoot\s+'common\.ps1'\)")

    def test_common_defines_required_functions(self):
        text = (HOST_DIR / "common.ps1").read_text(encoding="ascii")
        for name in COMMON_FUNCTIONS:
            self.assertRegex(text, rf"function\s+{re.escape(name)}\b")

    def test_product_scripts_are_function_only(self):
        products = HOST_DIR / "products"
        for path in sorted(products.glob("*.ps1")) if products.exists() else []:
            text = path.read_text(encoding="ascii")
            self.assertRegex(text, r"function\s+Invoke-Product\b", path)
            code_lines = []
            for line in text.splitlines():
                stripped = line.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                code_lines.append(stripped)
            self.assertTrue(code_lines, path)
            self.assertTrue(code_lines[0].startswith("function"), path)


if __name__ == "__main__":
    unittest.main()
