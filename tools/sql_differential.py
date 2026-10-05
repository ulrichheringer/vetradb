#!/usr/bin/env python3
"""Independent text protocol client for pinned PostgreSQL SQL result/SQLSTATE oracle.
Use only with the isolated trusted loopback fixture server, never production credentials.
"""
import argparse,json,socket,struct,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def exact(s,n):
    if n<0 or n>16*1024*1024:raise ValueError('packet budget')
    data=b''
    while len(data)<n:
        chunk=s.recv(n-len(data))
        if not chunk:raise ValueError('short packet')
        data+=chunk
    return data
def read(s):
    tag=exact(s,1);n=struct.unpack('!I',exact(s,4))[0]
    return tag,exact(s,n-4)
def drain(s):
    rows=[];oids=[];error=None
    while True:
        tag,b=read(s)
        if tag==b'T':
            count=struct.unpack('!H',b[:2])[0];p=2;oids=[];rows=[]
            for _ in range(count):
                p=b.index(b'\0',p)+1;oids.append(struct.unpack('!I',b[p+6:p+10])[0]);p+=18
        elif tag==b'D':
            count=struct.unpack('!H',b[:2])[0];p=2;row=[]
            for _ in range(count):
                n=struct.unpack('!i',b[p:p+4])[0];p+=4
                if n==-1:row.append(None)
                else:row.append(b[p:p+n].decode('utf-8'));p+=n
            rows.append(row)
        elif tag==b'E':
            p=0
            while p<len(b) and b[p]:
                kind=b[p:p+1];p+=1;end=b.index(b'\0',p)
                if kind==b'C':error=b[p:end].decode()
                p=end+1
        elif tag==b'R' and b!=bytes(4):raise ValueError('reference must use isolated local trust authentication')
        elif tag==b'Z':return {'error':error} if error else {'rows':rows,'oids':oids}
def connect(host,port):
    if host not in ('127.0.0.1','localhost'):raise ValueError('reference must be loopback')
    s=socket.create_connection((host,port),timeout=30)
    b=struct.pack('!I',196608)+b'user\0postgres\0database\0reference\0client_encoding\0UTF8\0\0'
    s.sendall(struct.pack('!I',len(b)+4)+b);drain(s);return s
def main():
    p=argparse.ArgumentParser();p.add_argument('--host',default='127.0.0.1');p.add_argument('--port',type=int,default=15439);p.add_argument('--record',action='store_true');p.add_argument('--reset',action='store_true');a=p.parse_args()
    cases=json.loads((ROOT/'docs/fixtures/sql/postgresql-17.json').read_text())
    with connect(a.host,a.port) as s:
        s.sendall(b'Q'+struct.pack('!I',4+len(b'SHOW server_version\0'))+b'SHOW server_version\0');version=drain(s)['rows'][0][0]
        if not version.startswith('17.'):raise ValueError('PostgreSQL 17 is required')
        if a.reset:
            b=b'DROP SCHEMA public CASCADE; CREATE SCHEMA public;\0';s.sendall(b'Q'+struct.pack('!I',len(b)+4)+b);drain(s)
        expected=[]
        for sql in cases:
            b=sql.encode()+b'\0';s.sendall(b'Q'+struct.pack('!I',len(b)+4)+b);expected.append(drain(s))
    actual=json.loads(subprocess.check_output(['cargo','run','--quiet','--locked','-p','vetra-engine','--bin','sql-fixture'],input=json.dumps(cases).encode(),cwd=ROOT))
    failures=[]
    for i,(sql,want,got) in enumerate(zip(cases,expected,actual)):
        if want!=got:failures.append({'case':i,'sql':sql,'reference':want,'vetra':got})
    if failures:raise SystemExit(json.dumps(failures,indent=2))
    if a.record:(ROOT/'docs/fixtures/sql/postgresql-17-results.json').write_text(json.dumps({'version':version,'results':expected},indent=2)+'\n')
    print(f'PostgreSQL {version}: {len(cases)} result/NULL/OID/SQLSTATE fixtures agree.')
if __name__=='__main__':main()
