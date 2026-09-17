#!/usr/bin/python3
import json, pathlib, sys, time
root = pathlib.Path(__file__).parent
mode = 'FIXTURE_MODE'
threads = {}
turn = 0
stalled = False
def emit(value):
    print(json.dumps(value), flush=True)
def status(thread, value):
    emit({'method':'thread/status/changed','params':{'threadId':thread,'status':{'type':value}}})
for line in sys.stdin:
    with (root / 'requests.jsonl').open('a') as f:
        f.write(line)
    msg = json.loads(line)
    method, p = msg.get('method'), msg.get('params', {})
    if 'id' not in msg: continue
    if method == 'initialize':
        result = {'userAgent':'codex-cli ' + ('0.999.0' if mode == 'unsupported' else '0.153.4')}
    elif method == 'thread/start':
        if mode == 'reject-start':
            emit({'id':msg['id'],'error':{'code':-32600,'message':'PRIVATE CONFIG ERROR'}})
            continue
        tid = 'thread-' + str(len(threads) + 1)
        threads[tid] = p.get('config', {})
        result = {'thread':{'id':tid,'sessionId':'session-'+tid,'status':{'type':'idle'}},'model':'native-model','cwd':str(root)}
        status(tid, 'idle')
    elif method == 'thread/read':
        result = {'thread':{'id':p['threadId'],'status':{'type':'active' if mode == 'active' else 'idle'}}}
    elif method == 'thread/unsubscribe':
        result = {}
    elif method == 'thread/resume':
        tid = p['threadId']
        if 'config' in p:
            if mode == 'reject':
                emit({'id':msg['id'],'error':{'code':-32600,'message':'PRIVATE CONFIG ERROR'}})
                continue
            if mode != 'ignore':
                status('wrong-thread' if mode == 'wrong-owner' else tid, 'notLoaded')
                status(tid, 'idle')
            if mode == 'cancel-once' and not stalled:
                stalled = True
                (root / 'reload.ready').touch()
                while not (root / 'reload.release').exists(): time.sleep(0.005)
            threads[tid] = p['config']
        result = {'thread':{'id':tid,'sessionId':'session-'+tid,'status':{'type':'idle'}},'model':'native-model','cwd':str(root)}
    elif method == 'model/list':
        result = {'data':[{'model':'native-model','defaultReasoningEffort':'low','supportedReasoningEfforts':[{'reasoningEffort':'low'}]}],'nextCursor':None}
    elif method == 'config/read':
        result = {'config':{'model_reasoning_effort':'low'}}
    elif method == 'turn/start':
        turn += 1
        result = {'turn':{'id':f'turn-{turn}'}}
    else: result = {}
    emit({'id':msg['id'],'result':result})
    if method == 'turn/start':
        if mode == 'compaction-events':
            for phase in ['started','completed']:
                emit({'method':'item/'+phase,'params':{'threadId':p['threadId'],'turnId':f'turn-{turn}','item':{'type':'contextCompaction','id':'boundary'}}})
            emit({'method':'item/completed','params':{'threadId':p['threadId'],'turnId':'stale-turn','item':{'type':'contextCompaction','id':'stale-boundary'}}})
            emit({'method':'thread/compacted','params':{'threadId':p['threadId'],'turnId':f'turn-{turn}'}})
        if mode == 'hold-root' or (mode == 'other-active' and p['threadId'] == 'thread-2'): continue
        if mode in ['live-child','compaction-events']:
            emit({'method':'thread/started','params':{'thread':{'id':'child','parentThreadId':p['threadId'],'source':{'subAgent':{'thread_spawn':{'parent_thread_id':p['threadId'],'depth':1}}}}}})
            emit({'method':'turn/started','params':{'threadId':'child','turn':{'id':'child-turn'}}})
            if mode == 'compaction-events':
                for phase in ['started','completed']:
                    emit({'method':'item/'+phase,'params':{'threadId':'child','turnId':'child-turn','item':{'type':'contextCompaction','id':'child-boundary'}}})
                emit({'method':'turn/completed','params':{'threadId':'child','turn':{'id':'child-turn','status':'completed'}}})
        emit({'method':'turn/completed','params':{'threadId':p['threadId'],'turn':{'id':f'turn-{turn}','status':'completed'}}})
