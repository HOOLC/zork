#!/usr/bin/env python3
"""Real Agent/Station contract for immutable local file submissions."""
import json
import os
import subprocess
import unittest
from urllib.request import urlopen

from test_station_entry import StationEntryContractTest, TARGET


class DriveArtifactContractTest(StationEntryContractTest):
    def test_z_file_submissions_versions_restart_and_offline_bytes(self):
        session_id = self.create_session()
        self.assertEqual(self.request('POST', f'/v1/im/sessions/{session_id}/messages', {'content': 'Prepare a report file'})[0], 202)
        _, tasks = self.request('GET', '/v1/tasks')
        task = next(t for t in tasks['items'] if t['session_id'] == session_id)
        _, context = self.request('GET', f'/v1/tools/context?threadId={session_id}')
        path = self.workspace / 'report.md'
        first_bytes = '# 第一版报告\n\n原始快照。\n'.encode()
        path.write_bytes(first_bytes)
        body={'sessionKey':context['sessionKey'],'conversationId':context['conversationId'],'rootMessageId':context['rootMessageId'],'filePath':str(path),'initialComment':'First file submission'}
        status,result=self.request('POST','/chat/post-file',body)
        self.assertEqual(status,200,result)
        first=result['artifact']
        self.assertEqual((first['task_id'], first['version'], first['source_path']), (task['task_id'], 1, 'report.md'))
        self.assertEqual(first['caption'], 'First file submission')
        status,result=self.request('POST','/chat/post-file',body)
        self.assertEqual(status,200,result)
        duplicate=result['artifact']
        self.assertEqual(duplicate['artifact_id'], first['artifact_id'])
        path.write_text('# 第二版报告\n\n更新后的内容。\n')
        status, response = self.request('POST', f"/v1/tasks/{task['task_id']}/artifacts", {'path': str(path)})
        self.assertEqual(status, 200)
        second = response['artifact']
        self.assertEqual(second['version'], 2)
        self.assertNotEqual(second['artifact_id'], first['artifact_id'])
        # Destination validation remains mandatory for session-bound file delivery.
        self.assertEqual(self.request('POST', '/chat/post-file', {'sessionKey': context['sessionKey'], 'conversationId': 'wrong', 'rootMessageId': context['rootMessageId'], 'filePath': str(path)})[0], 400)
        self.assertEqual(self.request('POST', f"/v1/tasks/{task['task_id']}/artifacts", {'path': '/etc/hosts'})[0], 400)
        path.unlink()
        cls = type(self)
        command = cls.station.args
        cls.stop_process(cls.station)
        cls.station = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        cls.wait_http(cls.station_url, '/readyz')
        # Agent now runs inside the station; the deleted source proves snapshot independence.
        _, artifacts = self.request('GET', '/v1/artifacts')
        self.assertEqual({a['artifact_id'] for a in artifacts['items']}, {first['artifact_id'], second['artifact_id']})
        self.assertTrue(all('content' not in a for a in artifacts['items']))
        with urlopen(f"{self.station_url}/v1/artifacts/{first['artifact_id']}/content") as response:
            self.assertEqual(response.read(), first_bytes)
            self.assertEqual(response.headers['Content-Disposition'], 'attachment')
        _, detail = self.request('GET', f"/v1/tasks/{task['task_id']}")
        self.assertEqual(len(detail['artifacts']), 2)
        self.assertEqual(len(self.messages(session_id)), 2, 'explicit file delivery appears once; retries must not duplicate its message')


if __name__ == '__main__':
    unittest.main(defaultTest='DriveArtifactContractTest')
