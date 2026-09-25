import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("deployment", Path(__file__).parents[1] / "deploy.py")
deployment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deployment)


class DeploymentTests(unittest.TestCase):
    @patch.object(deployment, "wrangler_authenticated", return_value=False)
    def test_private_google_configuration_is_never_copied_into_public_worker_config(self, _authenticated):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            google = root / "google.json"
            deployment.write_private(google, {"web": {
                "client_id": "fixture.apps.googleusercontent.com", "client_secret": "private-test-value",
                "redirect_uris": ["https://relay.zork.ing/v1/auth/google/callback"],
            }})
            config = root / "wrangler.json"
            config.write_text(json.dumps({
                "account_id": "test-account",
                "routes": [{"pattern": "relay.zork.ing", "custom_domain": True}],
                "vars": {"AUTH_SIGNING_KEY": "must-not-be-copied"},
                "durable_objects": {"bindings": []},
            }))
            args = SimpleNamespace(config=config, google_client=google, token_file=root / "token")
            public, private, missing = deployment.configuration(args)
            self.assertNotIn("private-test-value", json.dumps(public))
            self.assertNotIn("must-not-be-copied", json.dumps(public))
            self.assertEqual(private["GOOGLE_CLIENT_SECRET"], "private-test-value")
            self.assertIn({"name": "RELAY_HUB", "class_name": "RelayHub"}, public["durable_objects"]["bindings"])
            self.assertNotIn("containers", public)
            self.assertTrue(any("Cloudflare API token" in item for item in missing))
            google.chmod(0o644)
            with self.assertRaisesRegex(ValueError, "mode 600"):
                deployment.configuration(args)

    @patch.object(deployment, "wrangler_authenticated", return_value=True)
    def test_relay_deployment_does_not_require_google_configuration(self, _authenticated):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "wrangler.json"
            config.write_text(json.dumps({"account_id": "test-account"}))
            args = SimpleNamespace(config=config, google_client=root / "missing.json", token_file=root / "token")
            public, private, missing = deployment.configuration(args)
            self.assertEqual(missing, [])
            self.assertNotIn("GOOGLE_CLIENT_SECRET", private)
            self.assertEqual(public["vars"]["GOOGLE_CLIENT_ID"], "")

    def test_a_changed_candidate_is_rejected_before_any_deploy_operation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            file = root / "worker.js"
            file.write_text("original")
            (root / "manifest.json").write_text(json.dumps({"files": {"worker.js": deployment.digest(file)}}))
            file.write_text("changed")
            with self.assertRaisesRegex(ValueError, "Candidate file changed"):
                deployment.deploy(SimpleNamespace(output=root), {}, [])


if __name__ == "__main__":
    unittest.main()
