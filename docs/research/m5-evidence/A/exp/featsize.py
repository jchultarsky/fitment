import subprocess, time, os, sys, json, glob
root='/Users/julian/projects/fitment'
files=[]
for d in ['corpus/out','fixtures']:
    for dp,dn,fn in os.walk(os.path.join(root,d), followlinks=True):
        for f in fn:
            if f.lower().endswith(('.stp','.step','.p21')): files.append(os.path.join(dp,f))
files.sort()
rows=[]
for f in files:
    t=time.perf_counter()
    p=subprocess.run([root+'/target/release/fitment','features',f],capture_output=True)
    dt=time.perf_counter()-t
    out=p.stdout
    st='?'
    try: st=json.loads(out)['status']
    except Exception: st='err%d'%p.returncode
    rows.append((os.path.getsize(f),len(out),dt,st,p.returncode,f))
acc=[r for r in rows if r[3]=='accepted']
ref=[r for r in rows if r[3]=='refused']
oth=[r for r in rows if r[3] not in ('accepted','refused')]
print('files',len(rows),'accepted',len(acc),'refused',len(ref),'other',len(oth))
import statistics as s
for name,sel in [('all',rows),('acc',acc)]:
    st=sum(r[0] for r in sel); fj=sum(r[1] for r in sel)
    print(name,'step MB %.1f feat MB %.1f ratio %.2f'%(st/1e6,fj/1e6,fj/st))
    ts=sorted(r[2] for r in sel); print(' time median %.3f p95 %.3f max %.3f total %.1f'%(s.median(ts),ts[int(.95*len(ts))],ts[-1],sum(ts)))
big=sorted(rows,key=lambda r:-r[1])[:5]
for r in big: print('big feat',r[0],r[1],'%.2f'%r[2],r[3],os.path.basename(r[5]))
slow=sorted(rows,key=lambda r:-r[2])[:5]
for r in slow: print('slow',r[0],r[1],'%.2f'%r[2],r[3],os.path.basename(r[5]))
for r in oth: print('other',r)
