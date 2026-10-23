#!/usr/bin/env python3
"""Native TLS acceptance receipts. Python3.11+, Rust/Cargo, cargo-audit and a fetched RustSec Git DB.
No live service, production key/token/history, or hardware readings are accessed.
"""
import argparse,gzip,hashlib,json,os,re,signal,subprocess,sys,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--audit-bin',required=True,type=Path)
    p.add_argument('--advisory-db',required=True,type=Path)
    p.add_argument('--out',type=Path,default=Path(__file__).resolve().parent/'receipts')
    a=p.parse_args();a.out.mkdir(parents=True,exist_ok=False)
    subprocess.run(['git','diff','--exit-code','HEAD','--','src','Cargo.toml','Cargo.lock'],cwd=ROOT,check=True)
    manifest={'source':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        'original_pr9':'3197d3b2851a60008302b5b5c9c0d82e8ae93cbb',
        'baseline_main':subprocess.check_output(['git','rev-parse','origin/main'],cwd=ROOT,text=True).strip(),
        'scope':'Real loopback TLS/HTTP and actual temporary-file I/O with explicitly synthetic reading; no hardware measurement, production data, deployment or service restart',
        'results':[], 'complete':False}
    def run(name,args,expected=0,env=None):
        proc=subprocess.Popen([str(x) for x in args],cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,start_new_session=True)
        try: stdout,stderr=proc.communicate(timeout=180);code=proc.returncode
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid,signal.SIGTERM)
            try:stdout,stderr=proc.communicate(timeout=5)
            except subprocess.TimeoutExpired:os.killpg(proc.pid,signal.SIGKILL);stdout,stderr=proc.communicate()
            code=124
        files={}
        for stream,data in [('stdout',stdout),('stderr',stderr)]:
            filename=name+'.'+stream+'.gz';(a.out/filename).write_bytes(gzip.compress(data,mtime=0));files[stream]={'file':filename,'sha256':hashlib.sha256(data).hexdigest()}
        row={'name':name,'command':[str(x) for x in args],'exit':code,'expected_exit':expected,'streams':files}
        manifest['results'].append(row);(a.out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
        assert code==expected,(name,code,(stdout+stderr).decode(errors='replace'))
        if name=='audit':assert b'error:' not in stderr.lower(),stderr.decode(errors='replace')
        return stdout
    env=dict(os.environ,TMPDIR=os.environ.get('TMPDIR',str(ROOT.parent)),CARGO_BUILD_JOBS='2')
    manifest['rustc']=run('rustc',['rustc','--version'],env=env).decode().strip()
    manifest['cargo']=run('cargo',['cargo','--version'],env=env).decode().strip()
    run('fmt',['cargo','fmt','--all','--','--check'],env=env)
    tests=run('tests',['cargo','test','--locked','--','--nocapture'],env=env).decode()
    match=re.search(r'test result: ok\. (\d+) passed; (\d+) failed',tests);assert match and int(match.group(2))==0
    manifest['test_counts']={'passed':int(match.group(1)),'failed':int(match.group(2))}
    assert 'TLSv1_2: real TLS reading200' in tests and 'TLSv1_3: real TLS reading200' in tests
    assert 'TLS1.0: actual legacy ClientHello refused:' in tests and 'TLS1.1: actual legacy ClientHello refused:' in tests
    run('clippy',['cargo','clippy','--locked','--all-targets','--','-D','warnings'],env=env)
    run('release',['cargo','build','--locked','--release'],env=env)
    run('dependency-tree',['cargo','tree','--locked','-i','rustls'],env=env)
    with tempfile.TemporaryDirectory(prefix='L645-floor-',dir=env['TMPDIR']) as temp:
        before=Path(temp)/'before.lock';before.write_bytes(subprocess.check_output(['git','show',manifest['baseline_main']+':Cargo.lock'],cwd=ROOT))
        r=json.loads(run('lock-floor-red',[sys.executable,Path(__file__).with_name('check-lock.py'),before],expected=1,env=env))
        assert r['pass'] is False and r['refused_versions']==['0.20.9']
    r=json.loads(run('lock-floor-green',[sys.executable,Path(__file__).with_name('check-lock.py'),ROOT/'Cargo.lock'],env=env));assert r['pass']
    manifest['rustls_versions']=r['rustls_versions']
    # Isolate scanner configuration: no user ignore file or advisory exclusions.
    manifest['advisory_db_git_sha']=subprocess.check_output(['git','-C',str(a.advisory_db),'rev-parse','HEAD'],text=True).strip()
    with tempfile.TemporaryDirectory(prefix='L645-audit-home-',dir=env['TMPDIR']) as temp:
        audit_env=dict(env,CARGO_HOME=temp)
        run('audit-registry-bootstrap',['cargo','fetch','--locked','--manifest-path',ROOT/'Cargo.toml'],env=audit_env)
        manifest['audit_version']=run('audit-version',[a.audit_bin,'audit','--version'],env=audit_env).decode().strip()
        audit=json.loads(run('audit',[a.audit_bin,'audit','--db',a.advisory_db,'--no-fetch','--file',ROOT/'Cargo.lock','--json'],env=audit_env))
        assert audit['settings']['ignore']==[] and audit['vulnerabilities']['count']==0
    assert subprocess.check_output(['git','-C',str(a.advisory_db),'rev-parse','HEAD'],text=True).strip()==manifest['advisory_db_git_sha']
    original=subprocess.check_output(['git','show',manifest['original_pr9']+':src/main.rs'],cwd=ROOT,text=True)
    current=(ROOT/'src/main.rs').read_text()
    paths={}
    for name in ['HISTORY','TOKEN','CERT','KEY']:
        pattern=rf'const\s+{name}:\s*&str\s*=\s*"([^"]+)"'
        old=re.search(pattern,original);new=re.search(pattern,current);assert old and new and old.group(1)==new.group(1),name
        paths[name]=new.group(1)
    manifest['production_paths_unchanged']=paths
    manifest['files']={str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [ROOT/'Cargo.toml',ROOT/'Cargo.lock',ROOT/'src/main.rs',ROOT/'src/tls.rs',ROOT/'target/release/readings-window']}
    manifest['complete']=True
    (a.out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print(json.dumps(manifest,indent=2))
if __name__=='__main__':main()
