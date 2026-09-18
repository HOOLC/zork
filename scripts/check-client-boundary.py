#!/usr/bin/env python3
"""Check mechanical core/UI boundaries. Business semantics still require review."""
import argparse
from pathlib import Path
import re
import sys
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
UI = ("zork-ui", "zork-gui", "zork-gui-web")
SERVICES = {"reqwest", "rusqlite", "zork-config", "zork-mesh", "zork-browser",
            "openidconnect", "zork-station", "zork-agent", "zork-profile"}
# Preserve offsets so diagnostics point to the real source line. String literals
# remain available for route checks but cannot imitate a cfg(test) declaration.
TOKENS = re.compile(r'r(?P<hashes>#{0,16})"[\s\S]*?"(?P=hashes)|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|//[^\n]*|/\*[\s\S]*?\*/')


def blank(value):
    return re.sub(r"[^\n]", " ", value)


def mask(source, strings=True):
    return TOKENS.sub(lambda m: blank(m[0]) if strings or m[0].startswith(("//", "/*")) else m[0], source)


def production_source(source):
    syntax = mask(source)
    regions = []
    for match in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", syntax):
        depth, end = 1, match.end()
        while depth and end < len(syntax):
            depth += (syntax[end] == "{") - (syntax[end] == "}")
            end += 1
        regions.append((match.start(), end))
    for start, end in reversed(regions):
        source = source[:start] + blank(source[start:end]) + source[end:]
    return source


def violations(source, kotlin=False, platform_io=False):
    source = production_source(source)
    code, literals = mask(source), mask(source, strings=False)
    rules = [
        (code, r"\b(?:reqwest|rusqlite|zork_config|zork_mesh|openidconnect)::", "service implementation imported by UI"),
        (code, r"\bnode_request\s*\(", "raw Station request in UI"),
        (code, r"\b(?:Command|Self)::Request\s*\{", "raw request intent in UI"),
        (literals, r"/v1/(?:im|node|agent|tasks|profiles|mesh)(?:[/\"?])", "Station business route in UI"),
        (code, r"\backnowledgedMessages\b", "second delivery reducer in UI"),
    ]
    if not platform_io:
        rules += [
            (code, r"\b(?:std|tokio)::(?:fs|net)\b|\bstd::process::Command\b", "IO implemented by a view"),
            (code, r"\bClientStore::open\s*\(|\bstore\s*\.\s*(?:put|get|save_node|remove_node|enqueue)\s*(?:\([^)]*|::<)", "raw persistent storage used by a view"),
            (code, r"\bgetSharedPreferences\s*\(|\b(?:openInputStream|openOutputStream)\s*\(", "platform persistence used by a view"),
        ]
    if kotlin:
        rules += [
            (code, r"\b(?:okhttp3|retrofit2|java\.net)\.", "network implementation in Android UI/bridge"),
            (literals, r'\b(?:repo\.)?command\s*\(\s*"(?:request|read|compose)"', "legacy transport/whole-draft command in UI"),
        ]
    return [(source.count("\n", 0, match.start()) + 1, reason)
            for text, pattern, reason in rules for match in re.finditer(pattern, text)]


def dependencies(manifest):
    tables = [manifest.get("dependencies", {})]
    tables += [target.get("dependencies", {}) for target in manifest.get("target", {}).values()]
    return {value.get("package", name) if isinstance(value, dict) else name
            for table in tables for name, value in table.items()}


def fixture_roots(package, manifest):
    roots = []
    for binary in manifest.get("bin", []):
        if "headless-bench" in binary.get("required-features", []):
            path = package / binary["path"]
            roots += [path, path.with_suffix("")]
    # Honor actual source module gates; do not exempt an entire application tree.
    for path in (package / "src").rglob("*.rs"):
        syntax = mask(path.read_text(), strings=False)
        for match in re.finditer(r'#\[cfg\(feature\s*=\s*"headless-bench"\)\]\s*(?:pub\s+)?mod\s+(\w+)\s*;', syntax):
            base = path.parent if path.stem in {"lib", "mod", "main"} else path.with_suffix("")
            roots += [base / (match[1] + ".rs"), base / match[1]]
    return roots


def check(root):
    failures = []
    for name in UI:
        package = root / "crates" / name
        manifest = tomllib.loads((package / "Cargo.toml").read_text())
        forbidden = SERVICES | ({"zork-client-core"} if name == "zork-ui" else set())
        for dependency in sorted(dependencies(manifest) & forbidden):
            failures.append(f"{package.relative_to(root)}/Cargo.toml: production dependency {dependency}")
        fixtures = fixture_roots(package, manifest)
        for path in (package / "src").rglob("*.rs"):
            if any(path == fixture or fixture in path.parents for fixture in fixtures):
                continue
            # Automation implements platform input, screenshot and profiler
            # artifact ports. Business routes and Station calls remain forbidden there.
            platform_io = name in {"zork-ui", "zork-gui"} and "automation" in path.relative_to(package / "src").parts
            for line, reason in violations(path.read_text(), platform_io=platform_io):
                failures.append(f"{path.relative_to(root)}:{line}: {reason}")
    core = root / "crates/zork-client-core"
    forbidden = {"zork-ui", "zork-gui", "zork-gui-web"}
    for dependency in dependencies(tomllib.loads((core / "Cargo.toml").read_text())):
        if dependency in forbidden or dependency.startswith("gpui"):
            failures.append(f"crates/zork-client-core/Cargo.toml: UI dependency {dependency}")
    for path in (root / "crates/zork-gui-web/src").rglob("*.rs"):
        if re.search(r'#\[path\s*=\s*"[^"\n]*zork-client-core/', mask(path.read_text(), strings=False)):
            failures.append(f"{path.relative_to(root)}: imports private core source instead of public contracts")
    for path in (root / "apps/android/app/src/main/java").rglob("*.kt"):
        # JNI's narrow OS port supplies URI bytes and legacy preference input.
        for line, reason in violations(path.read_text(), kotlin=True, platform_io=path.name == "NativeBridge.kt"):
            failures.append(f"{path.relative_to(root)}:{line}: {reason}")
    return sorted(set(failures))


class BoundaryTests(unittest.TestCase):
    def test_business_call_is_detected_across_comments_and_lines(self):
        self.assertTrue(violations('client.node_request /* explanation */\n (method, path, body);'))

    def test_inline_tests_do_not_hide_later_production_code(self):
        source = '#[cfg(test)] mod tests { fn fixture() { std::fs::write("}", "x"); } }\nfn view() { std::fs::read(path); }'
        found = violations(source)
        self.assertEqual(found, [(2, "IO implemented by a view")])

    def test_comments_and_literals_cannot_disable_guard(self):
        self.assertFalse(violations('// reqwest::Client\nlet label = "node_request()";'))
        self.assertTrue(violations('let fake = "#[cfg(test)] mod tests {"; client.node_request(method, path, body);'))

    def test_renaming_a_dependency_does_not_bypass_manifest_check(self):
        self.assertEqual(dependencies({"dependencies": {"http_alias": {"package": "reqwest"}}, "dev-dependencies": {"rusqlite": "1"}}), {"reqwest"})

    def test_os_port_is_not_a_station_escape_hatch(self):
        self.assertFalse(violations('resolver.openInputStream(uri)', kotlin=True, platform_io=True))
        self.assertTrue(violations('repo.command("request", peer)', kotlin=True, platform_io=True))
        self.assertTrue(violations('client.node_request(method, path, body)', platform_io=True))

    def test_views_can_submit_business_intents_and_map_snapshots(self):
        self.assertFalse(violations('source.save_model(id, input); self.rows = snapshot.rows.clone();'))
        self.assertFalse(violations('actions.perform("save_model", input)', kotlin=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(BoundaryTests))
        return 0 if result.wasSuccessful() else 1
    failures = check(args.root.resolve())
    if failures:
        print("\n".join(failures))
        return 1
    print("PASS client boundary: UI dependencies, IO/transport calls, Android intents and public Web core imports")
    return 0


if __name__ == "__main__":
    sys.exit(main())
