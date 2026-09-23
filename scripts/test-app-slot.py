import importlib.util
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).parent / 'lib'))
import test_app_slot as slot


class AppSlotTest(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)
        self.addCleanup(patch.stopall)
        patch.object(slot,'remove_app',side_effect=lambda p:shutil.rmtree(p) if p.exists() else None).start()
        patch.object(slot,'unregister_app').start()
        self.stop=patch.object(slot,'stop_app',return_value=False).start()

    def build(self, value):
        with slot.app_slot(self.root) as update:
            update.staged.mkdir();(update.staged/'version').write_text(value)
            return update.publish()

    def test_update_keeps_old_until_publish_then_replaces_same_path(self):
        current=self.build('old');self.stop.reset_mock()
        with slot.app_slot(self.root) as update:
            self.assertEqual((current/'version').read_text(),'old')
            self.stop.assert_not_called()
            update.staged.mkdir();(update.staged/'version').write_text('new')
            self.assertEqual(update.publish(),current)
        self.assertEqual((current/'version').read_text(),'new')
        self.assertEqual(list(current.parent.glob('*.app')),[current])
        self.assertFalse((current.parent/'.previous').exists())
        self.assertFalse((current.parent/'.incoming').exists())

    def test_build_failure_preserves_old_and_does_not_stop_it(self):
        current=self.build('old');self.stop.reset_mock()
        with self.assertRaises(ValueError):
            with slot.app_slot(self.root) as update:
                update.staged.mkdir();raise ValueError('failed')
        self.stop.assert_not_called()
        self.assertEqual((current/'version').read_text(),'old')
        self.assertFalse((current.parent/'.incoming').exists())

    def test_stop_failure_preserves_old(self):
        current=self.build('old');self.stop.side_effect=RuntimeError('not exiting')
        with self.assertRaisesRegex(RuntimeError,'not exiting'):self.build('new')
        self.assertEqual((current/'version').read_text(),'old')

    def test_promote_failure_rolls_back_old(self):
        current=self.build('old');real=Path.rename
        def fail_new(path, destination):
            if path.parent.name=='.incoming':raise OSError('promotion failed')
            return real(path,destination)
        with patch.object(Path,'rename',fail_new),self.assertRaises(OSError):self.build('new')
        self.assertEqual((current/'version').read_text(),'old')

    def test_restart_only_if_previously_running(self):
        current=self.build('old');self.stop.return_value=True
        with patch.object(slot.subprocess,'run') as run:self.build('new')
        run.assert_called_once_with(['open','-n','-a',str(current)],check=True)

    def test_lock_only_blocks_other_update_not_existing_app(self):
        current=self.build('old')
        with slot.app_slot(self.root):
            self.assertTrue(current.exists())
            with self.assertRaisesRegex(RuntimeError,'already has'):
                with slot.app_slot(self.root):pass

    def test_worktrees_independent(self):
        with slot.app_slot(self.root/'a') as a,slot.app_slot(self.root/'b') as b:
            self.assertNotEqual(a.current,b.current)

    def test_interrupted_swap_recovery(self):
        root=self.root/'.tmp/macos-app.noindex'
        backup=root/'.previous/Zork.app';backup.mkdir(parents=True);(backup/'version').write_text('old')
        with slot.app_slot(self.root) as update:
            self.assertEqual((update.current/'version').read_text(),'old')


class ProcessOwnershipTest(unittest.TestCase):
    def test_matches_exact_bundle_path_not_other_worktree(self):
        app=Path('/tmp/tree one/Zork.app')
        output='12 /tmp/tree one/Zork.app/Contents/MacOS/zork-gui\n13 /tmp/tree two/Zork.app/Contents/MacOS/zork-gui\n14 /Applications/Zork.app/Contents/MacOS/zork-gui\n'
        with patch.object(slot.subprocess,'check_output',return_value=output):
            self.assertEqual(list(slot.app_processes(app)),[12])

    def test_test_command_failure_is_reported(self):
        with self.assertRaises(subprocess.CalledProcessError):
            slot.run_test([sys.executable,'-c','raise SystemExit(7)'],Path('/tmp/Zork.app'))


class PackagerTest(unittest.TestCase):
    def test_default_retains_app_without_archive_and_explicit_export_works(self):
        spec=importlib.util.spec_from_file_location('packager',Path(__file__).parent/'package-macos-client.py')
        packager=importlib.util.module_from_spec(spec);spec.loader.exec_module(packager)
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);real_slot=slot.app_slot
            def build(args,repo,app):
                self.assertEqual(args.channel, 'test')
                self.assertTrue(args.id_prefix.startswith('ing.zork-test.'))
                self.assertTrue(args.test_instance)
                app.mkdir();(app/'version').write_text('new')
            with patch.object(packager,'app_slot',side_effect=lambda repo:real_slot(root)),patch.object(packager,'build_app',side_effect=build),patch.object(slot,'stop_app',return_value=False),patch.object(slot,'unregister_app'),patch.object(slot,'remove_app',side_effect=lambda p:shutil.rmtree(p) if p.exists() else None):
                with patch('sys.argv',['package']):packager.main()
                self.assertTrue((root/'.tmp/macos-app.noindex/Zork.app/version').exists())
                self.assertEqual(list(root.rglob('*.tar.gz')),[])
                with patch('sys.argv',['package','--output',str(root/'export')]):packager.main()
                self.assertTrue((root/'export/Zork-macOS-arm64.tar.gz').exists())
                self.assertTrue((root/'.tmp/macos-app.noindex/Zork.app/version').exists())


if __name__=='__main__':unittest.main()
