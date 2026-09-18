#!/usr/bin/env python3
"""Client request IDs survive lost ACKs and Station/Agent restart."""
import json,subprocess
from test_station_entry import StationEntryContractTest
class ClientOutboxContractTest(StationEntryContractTest):
    def test_z_client_request_replay(self):
        session=self.create_session();route=f'/v1/im/sessions/{session}/messages'
        body={'request_id':'persisted-client-request','content':json.dumps({'fake_tool':{'name':'shell.run','input':{'command':"printf 'one\\n' >> client-executions.txt"}}})}
        self.assertEqual(self.request('POST',route,body)[0],202)
        self.wait_until(lambda:(self.workspace/'client-executions.txt').exists(),'first client request')
        self.assertEqual(self.request('POST',route,body)[0],202)
        self.assertEqual(self.request('POST',route,dict(body,content='changed'))[0],409)
        cls=type(self)
        for kind in ['station','agent']:
            process=getattr(cls,kind);command=process.args;cls.stop_process(process)
            setattr(cls,kind,subprocess.Popen(command,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL));cls.wait_http(getattr(cls,kind+'_url'),'/readyz')
        self.assertEqual(self.request('POST',route,body)[0],202)
        self.assertEqual(len(self.messages(session)),1)
        self.assertEqual((self.workspace/'client-executions.txt').read_text().splitlines(),['one'])
