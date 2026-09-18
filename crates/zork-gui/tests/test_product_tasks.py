#!/usr/bin/env python3
"""Real-process contract for durable local product tasks and human review."""
import json
import subprocess
import unittest

from test_station_entry import StationEntryContractTest


class ProductTaskContractTest(StationEntryContractTest):
    def task_for(self, session_id):
        status, body = self.request('GET', '/v1/tasks')
        self.assertEqual(status, 200)
        return next(t for t in body['items'] if t['session_id'] == session_id)

    def inbox_ids(self):
        status, body = self.request('GET', '/v1/inbox')
        self.assertEqual(status, 200)
        self.assertTrue(all(not t['goal'] and t['result_text'] is None for t in body['items']))
        return {t['task_id'] for t in body['items']}

    def deliver(self, session_id, text):
        _, context = self.request('GET', f'/cli/context?threadId={session_id}')
        status, _ = self.request('POST', '/chat/post-message', {
            'sessionKey': context['sessionKey'], 'conversationId': context['conversationId'],
            'rootMessageId': context['rootMessageId'], 'kind': 'final', 'text': text,
        })
        self.assertEqual(status, 200)

    def transition(self, task, action):
        return self.request('POST', f"/v1/tasks/{task['task_id']}/transitions", {
            'expected_revision': task['revision'], 'action': action,
        })

    def test_product_task_review_runs_restart_and_offline_reads(self):
        session_id = self.create_session()
        initial = self.task_for(session_id)
        self.assertNotEqual(initial['task_id'], session_id)
        self.assertEqual(initial['state'], 'open')
        status, _ = self.request('POST', f'/v1/im/sessions/{session_id}/messages', {'content': 'Review the project\nand report findings'})
        self.assertEqual(status, 202)
        self.wait_until(lambda: self.task_for(session_id)['last_run_status'] == 'finished', 'durable completed run')
        current = self.task_for(session_id)
        self.assertEqual(current['state'], 'open', 'turn completion must not complete the task')
        self.assertEqual(current['run_count'], 1)
        self.assertEqual(current['title'], 'Review the project and report findings')
        self.assertEqual(self.transition(current, 'accept')[0], 409)
        self.deliver(session_id, '## First result\n\nReady for review.')
        first_result = self.task_for(session_id)
        self.assertEqual(first_result['state'], 'review')
        self.assertIn(first_result['task_id'], self.inbox_ids())
        self.deliver(session_id, '## Corrected result\n\nPlease review this version.')
        status, failure = self.transition(first_result, 'accept')
        self.assertEqual((status, failure['error']), (409, 'task_revision_conflict'))
        current = self.task_for(session_id)
        status, accepted = self.transition(current, 'accept')
        self.assertEqual((status, accepted['state']), (200, 'completed'))
        self.assertEqual(accepted['result_message_id'], current['result_message_id'])
        self.assertNotIn(accepted['task_id'], self.inbox_ids())
        status, _ = self.request('POST', f'/v1/im/sessions/{session_id}/messages', {'content': 'Must not be delivered'})
        self.assertEqual(status, 409)
        self.assertEqual(len(self.messages(session_id)), 3)

        # Restart only the station. Its product DB and projected run IDs must survive replay.
        cls = type(self)
        command = cls.station.args
        cls.stop_process(cls.station)
        cls.station = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        cls.wait_http(cls.station_url, '/readyz')
        restored = self.task_for(session_id)
        self.assertEqual(restored['task_id'], accepted['task_id'])
        self.assertEqual(restored['state'], 'completed')
        self.assertEqual(restored['revision'], accepted['revision'])
        self.assertEqual(restored['run_count'], 1)
        status, detail = self.request('GET', f"/v1/tasks/{restored['task_id']}")
        self.assertEqual(status, 200)
        self.assertEqual(len(detail['runs']), 1)
        first_run_id = detail['runs'][0]['run_id']

        status, reopened = self.transition(restored, 'reopen')
        self.assertEqual((status, reopened['state']), (200, 'open'))
        self.assertIsNone(reopened['result_message_id'])
        status, _ = self.request('POST', f'/v1/im/sessions/{session_id}/messages', {'content': 'Check one more thing'})
        self.assertEqual(status, 202)
        self.wait_until(lambda: self.task_for(session_id)['run_count'] == 2 and self.task_for(session_id)['last_run_status'] == 'finished', 'second distinct run')
        current = self.task_for(session_id)
        self.assertEqual(current['task_id'], initial['task_id'])
        self.assertEqual(current['goal'], 'Review the project\nand report findings')
        _, detail = self.request('GET', f"/v1/tasks/{current['task_id']}")
        self.assertEqual(detail['runs'][1]['run_id'], first_run_id)
        self.assertNotEqual(detail['runs'][0]['run_id'], first_run_id)
        # Cancelling a product task must not silently leave a live run behind.
        status, _ = self.request('POST', f'/v1/im/sessions/{session_id}/messages', {
            'content': json.dumps({'fake_tool': {'name': 'shell.run', 'input': {'command': 'sleep 30'}}}),
        })
        self.assertEqual(status, 202)
        self.wait_until(lambda: self.task_for(session_id)['last_run_status'] == 'running', 'live third run')
        busy = self.task_for(session_id)
        self.assertNotIn(busy['task_id'], self.inbox_ids())
        self.assertEqual(self.transition(busy, 'cancel')[0], 409)
        self.assertEqual(self.request('POST', f'/v1/im/sessions/{session_id}/cancel')[0], 204)
        self.wait_until(lambda: self.task_for(session_id)['last_run_status'] == 'cancelled', 'runtime stop acknowledged')
        current = self.task_for(session_id)
        self.assertIn(current['task_id'], self.inbox_ids())
        status, cancelled = self.transition(current, 'cancel')
        self.assertEqual((status, cancelled['state']), (200, 'cancelled'))

        # Product data remains readable without the Agent. Decisions require a live idle check.
        pending_session = self.create_session()
        self.deliver(pending_session, 'Offline review candidate')
        pending = self.task_for(pending_session)
        cls.stop_process(cls.agent)
        self.assertIn(pending['task_id'], self.inbox_ids())
        self.assertNotIn(cancelled['task_id'], self.inbox_ids())
        offline = self.task_for(session_id)
        self.assertEqual(offline['state'], 'cancelled')
        self.assertEqual(self.request('GET', f"/v1/tasks/{offline['task_id']}")[0], 200)
        self.assertEqual(self.transition(offline, 'reopen')[0], 502)


if __name__ == '__main__':
    unittest.main(defaultTest='ProductTaskContractTest')
