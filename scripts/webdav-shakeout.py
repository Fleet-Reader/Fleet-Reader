#!/usr/bin/env python3
"""Private dataset copies only. Fresh PLAINTEXT spaces reach isolated local fixtures; never use a public server."""
import http.server, http.client, socket, threading, urllib.parse
import importlib.util, pathlib, os, subprocess, json, time, argparse, contextlib, tempfile, shutil
ROOT=pathlib.Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('capture',ROOT.parent/'day-webdav/scripts/capture.py');capture=importlib.util.module_from_spec(spec);spec.loader.exec_module(capture)
p=argparse.ArgumentParser();p.add_argument('--source',required=True);p.add_argument('--rounds',type=int,default=8);p.add_argument('--lifecycle',action='store_true');p.add_argument('--batch',action='store_true');p.add_argument('--epochs',type=int,default=5);p.add_argument('--writes',type=int,default=200);p.add_argument('--clients',type=int,default=6);p.add_argument('--server',choices=['apache','nextcloud'],default='apache');p.add_argument('--provider',choices=['native','reqwest'],default='native');p.add_argument('--faults',action='store_true');p.add_argument('--trace',action='store_true');p.add_argument('--password-file');p.add_argument('--output',required=True);p.add_argument("--url");p.add_argument("--username",default="day_fixture");a=p.parse_args()
out=pathlib.Path(a.output);out.mkdir(parents=True,exist_ok=True)
@contextlib.contextmanager
def server():
 if a.server=='apache':
  with capture.apache() as result:yield result
 else:yield (a.url or 'http://127.0.0.1:18443/remote.php/dav/files/day_fixture/','Nextcloud isolated fixture')
@contextlib.contextmanager
def fault_proxy(url):
 if not a.faults and not a.trace:
  yield url;return
 target=urllib.parse.urlsplit(url);seen=set();lock=threading.Lock();counts={'lost_ack':0,'partial_write':0,'corrupt_read':0,'status401':0,'status507':0,'status503':0,'status429':0};put_counts={}
 class Handler(http.server.BaseHTTPRequestHandler):
  protocol_version='HTTP/1.1'
  def log_message(self,*args):pass
  def forward(self):
   body=self.rfile.read(int(self.headers.get('Content-Length','0')))
   fault=None
   if a.faults and '/tx-' in self.path:
    actor=self.path.rsplit('/tx-',1)[1].split('.')[0]
    key=(actor,self.command)
    with lock:
     if key not in seen and self.command in ('PUT','GET'):
      if self.command=='PUT':
       seen.add(key);fault='partial_write' if len(seen)%2==0 else 'lost_ack'
      elif ('read',actor) not in seen and (actor,'PUT') in seen:
       seen.add(key);seen.add(('read',actor));fault='corrupt_read'
   if a.faults and '/tx-' in self.path and self.command=='PUT' and fault is None:
    with lock:
     count=put_counts.get(actor,0);put_counts[actor]=count+1
    if count<4:
     code=[401,507,503,429][count]
     with lock:counts[f'status{code}']+=1
     self.send_response(code);self.send_header('Content-Length','0');self.send_header('Retry-After','1');self.end_headers();return
   sent=body[:len(body)//2] if fault=='partial_write' else body
   h={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection','content-length','transfer-encoding')}
   conn=http.client.HTTPConnection(target.hostname,target.port,timeout=60);conn.request(self.command,self.path,sent,h);r=conn.getresponse();data=r.read();status=r.status
   if a.trace:
    with lock:
     with (out/'http-trace.jsonl').open('a') as trace:trace.write(json.dumps({'method':self.command,'path':self.path,'status':status,'response':data[:600].decode('utf8','replace') if status>=400 else ''})+'\n')
   if fault and status in (200,201,204):
    with lock:counts[fault]+=1
    if fault=='lost_ack':
     conn.close();self.close_connection=True;self.connection.shutdown(socket.SHUT_RDWR);self.connection.close();return
    if fault=='corrupt_read':data=b'synthetic corrupt transfer'
   self.send_response(status)
   for k,v in r.getheaders():
    if k.lower() not in ('content-length','connection','transfer-encoding'):self.send_header(k,v)
   self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data);conn.close()
  do_GET=do_PUT=do_MKCOL=do_PROPFIND=do_DELETE=forward
 srv=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=srv.serve_forever,daemon=True);thread.start()
 try:yield f'http://127.0.0.1:{srv.server_port}{target.path}'
 finally:
  srv.shutdown();srv.server_close();thread.join();(out/f'{a.server}-{a.provider}-faults.json').write_text(json.dumps(counts,indent=2)+'\n')
with server() as (upstream,version), fault_proxy(upstream) as url:
 env=os.environ.copy();env.update(DAV_FIXTURE_USER=a.username,DAY_NEWS_TEST_DB=str(pathlib.Path(a.source).resolve()),DAY_NEWS_TEST_ROUNDS=str(a.rounds),DAV_FIXTURE_URL=url,DAV_FIXTURE_PASSWORD=pathlib.Path(a.password_file).read_text().strip() if a.password_file else capture.SYNTHETIC_PASSWORD,DAV_PROVIDER=a.provider)
 if a.faults:env['DAY_NEWS_FAULT_TEST']='1'
 if a.batch:env['LIFECYCLE_BATCH']='1'
 if a.lifecycle:env.update(LIFECYCLE_EPOCHS=str(a.epochs),LIFECYCLE_WRITES=str(a.writes),LIFECYCLE_CLIENTS=str(a.clients))
 start=time.time();log=out/f'{a.server}-{a.provider}.log'
 with log.open('w') as f:
  r=subprocess.run(['cargo','test','--offline','-p','daynews-db','--features','experimental-webdav','--test','webdav','personal_lifecycle_stress' if a.lifecycle else 'personal_three_client_live_shakeout','--','--ignored','--nocapture'],cwd=ROOT,env=env,stdout=f,stderr=subprocess.STDOUT)
 if a.server=='apache':
  for f in pathlib.Path(tempfile.gettempdir()).glob('day-webdav-*/error.log'):
   shutil.copyfile(f,out/'apache-server.log')
 report={'server':version,'provider':a.provider,'rounds':a.rounds,'lifecycle':a.lifecycle,'batch':a.batch,'epochs':a.epochs if a.lifecycle else None,'clients':a.clients if a.lifecycle else None,'writes':a.writes if a.lifecycle else None,'elapsed_seconds':time.time()-start,'passed':r.returncode==0,'log':log.name,'dataset':'private SQLite backup; no source rows recorded in report'}
 (out/f'{a.server}-{a.provider}.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report));raise SystemExit(r.returncode)
