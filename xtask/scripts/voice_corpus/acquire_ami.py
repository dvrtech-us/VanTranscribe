#!/usr/bin/env python3
"""Bounded public AMI Range acquisition. Two origin requests at most; no vectors."""
if not __debug__: raise SystemExit('Run without Python optimization; verification checks are mandatory.')
import argparse, concurrent.futures, hashlib, io, json, os, pathlib, re, struct, threading, time, urllib.error, urllib.parse, urllib.request, wave, shutil
RATE=16000;CAP=1048576;HEADER=65536
NETWORK_BUDGET=310*1024*1024
network_reserved=0
def allowed_source(url,mid):
 expected=f'https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{mid}/audio/{mid}.Array1-01.wav'
 if url!=expected:raise ValueError('onlyofficialHTTPSAMIArray1-01sourcesallowed')
class NoRedirect(urllib.request.HTTPRedirectHandler):
 def redirect_request(self,*args,**kwargs):raise ValueError('sourceRedirectsarenotallowed')
urlopen=urllib.request.build_opener(NoRedirect()).open
lock=threading.Lock();progress={'headers':0,'ranges':0,'network_bytes':0,'cache_bytes':0,'sources_complete':0,'sources_failed':0}
def sha(data):return hashlib.sha256(data).hexdigest()
def dump(path,obj):
 path.parent.mkdir(parents=True,exist_ok=True);temp=path.with_suffix(path.suffix+'.part')
 data=(json.dumps(obj,indent=2,sort_keys=True)+'\n').encode()
 with temp.open('wb') as out:out.write(data);out.flush();os.fsync(out.fileno())
 os.replace(temp,path)
def store(path,data):
 path.parent.mkdir(parents=True,exist_ok=True);temp=path.with_suffix(path.suffix+'.part')
 with temp.open('wb') as out:out.write(data);out.flush();os.fsync(out.fileno())
 os.replace(temp,path)
def log(event,**extra):
 with lock:print(json.dumps({'event':event,**progress,**extra}),flush=True)
def geometry(data,total):
 if data[:4]!=b'RIFF' or data[8:12]!=b'WAVE':raise ValueError('notRIFFWAVE')
 pos=12;fmt=None;offset=None;size=None
 while pos+8<=len(data):
  tag,n=struct.unpack_from('<4sI',data,pos)
  if tag==b'fmt ':
   if n<16 or pos+24>len(data):raise ValueError('truncatedfmt')
   fmt=struct.unpack_from('<HHIIHH',data,pos+8)
  if tag==b'data':offset=pos+8;size=n;break
  pos+=8+n+n%2
 if fmt!=(1,1,RATE,32000,2,16) or offset is None or offset+size>total or size%2:raise ValueError('unexpectedgeometry')
 return {'encoding':'PCM16LE','channels':1,'sample_rate':RATE,'block_align':2,'bits_per_sample':16,'data_offset':offset,'data_bytes':size,'frames':size//2}
def request(url,start,end,etag=None):
 global network_reserved
 parsed=urllib.parse.urlsplit(url)
 if parsed.scheme!='https' or parsed.netloc!='groups.inf.ed.ac.uk' or parsed.query or parsed.fragment or not re.fullmatch(r'/ami/AMICorpusMirror/amicorpus/[A-Z]{2}[0-9]{4}[a-d]/audio/[A-Z]{2}[0-9]{4}[a-d]\.Array1-01\.wav',parsed.path):raise ValueError('sourceoriginnotallowed')
 n=end-start+1
 if not 0<n<=CAP:raise ValueError('requestexceedscap')
 headers={'Range':f'bytes={start}-{end}','Accept-Encoding':'identity','User-Agent':'VanTranscribe-Public-Corpus-Research/1'}
 if etag:headers['If-Range']=etag
 with lock:
  if progress['network_bytes']+network_reserved+n>NETWORK_BUDGET:raise ValueError('newpayloadbytebudgetexceeded')
  network_reserved+=n
 reserved=True
 try:
  for attempt in range(4):
   try:
    with urlopen(urllib.request.Request(url,headers=headers),timeout=45) as r:
     if r.geturl()!=url:raise ValueError('unexpectedsourceRedirect')
     if r.status!=206:raise ValueError('serverignoredrange_or_sourcechanged')
     match=re.fullmatch(r'bytes (\d+)-(\d+)/(\d+)',r.headers.get('Content-Range',''))
     if not match or tuple(map(int,match.groups()[:2]))!=(start,end):raise ValueError('wrongcontentrange')
     if r.headers.get('Content-Encoding','identity')!='identity':raise ValueError('encodedrange')
     if r.headers.get('Content-Length') and int(r.headers['Content-Length'])!=n:raise ValueError('wrongcontentlength')
     observed=r.headers.get('ETag')
     if not observed or observed.startswith('W/') or etag and observed!=etag:raise ValueError('missingweakorchangedETag')
     data=r.read(n+1)
     if len(data)!=n:raise ValueError('truncatedoroversizedrange')
     info={'source_url':url,'status':206,'content_range':r.headers['Content-Range'],'source_bytes':int(match.group(3)),'etag':observed,'last_modified':r.headers.get('Last-Modified'),'range_sha256':sha(data),'bytes':n}
    with lock:
     progress['network_bytes']+=len(data);network_reserved-=n;reserved=False
    time.sleep(.15)
    return data,info
   except (urllib.error.URLError,TimeoutError,OSError) as error:
    if attempt==3:raise
    delay=(2,6,14)[attempt]
    if isinstance(error,urllib.error.HTTPError):
     if error.code not in (408,425,429,500,502,503,504):raise
     try:delay=max(delay,min(60,int(error.headers.get('Retry-After','0'))))
     except ValueError:pass
    time.sleep(delay)
  raise RuntimeError('retryexhausted')
 finally:
  if reserved:
   with lock:network_reserved-=n

def cached_range(folder,name,url,start,end,etag=None):
 path=folder/(name+'.bin');meta=folder/(name+'.json')
 if path.exists() and meta.exists():
  info=json.loads(meta.read_text());data=path.read_bytes()
  if len(data)==end-start+1 and info['range_sha256']==sha(data) and info['source_url']==url and info['content_range']==f'bytes {start}-{end}/{info["source_bytes"]}' and (etag is None or info['etag']==etag):
   with lock:progress['cache_bytes']+=len(data)
   return data,info
  raise ValueError('cachepinmismatch')
 data,info=request(url,start,end,etag);store(path,data);dump(meta,info);return data,info
def union(items):
 out=[]
 for a,b in sorted(items):
  if out and a<=out[-1][1]:out[-1][1]=max(out[-1][1],b)
  else:out.append([a,b])
 return out
def coalesce(items):
 joined=[]
 for a,b in union(items):
  if joined and a-joined[-1][1]<=2*RATE:joined[-1][1]=b
  else:joined.append([a,b])
 result=[]
 for a,b in joined:
  while a<b:end=min(b,a+CAP//2);result.append([a,end]);a=end
 return result
def make_wav(pcm):
 b=io.BytesIO()
 with wave.open(b,'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(RATE);w.writeframes(pcm)
 return b.getvalue()
def source_job(mid,samples,output):
 if not re.fullmatch(r'[A-Z]{2}[0-9]{4}[a-d]',mid):raise ValueError('badmeetingID')
 folder=output/'ranges'/mid;url=samples[0]['source_url'];allowed_source(url,mid)
 if any(s['source_url']!=url for s in samples):raise ValueError('inconsistentsource')
 header,head=cached_range(folder,'header-0-65535',url,0,HEADER-1)
 fmt=geometry(header,head['source_bytes'])
 with lock:progress['headers']+=1
 intervals=coalesce([r for sample in samples for r in sample['source_windows_frames']]);chunks=[];audit=[]
 for a,b in intervals:
  if not 0<=a<b<=fmt['frames']:raise ValueError('selectedframeoutsideWAV')
 # Existing cached responses are reused only after independent byte/hash/origin
 # checks. They remain actual HTTP responses; assembled data is never mislabeled
 # as a new206 response.
 for metadata in sorted(folder.glob('pcm-*.json')):
  info=json.loads(metadata.read_text());match=re.fullmatch(r'bytes (\d+)-(\d+)/(\d+)',info['content_range'])
  if not match:raise ValueError('invalidcachedrange')
  start,end,total=map(int,match.groups());a=(start-fmt['data_offset'])//2;b=(end+1-fmt['data_offset'])//2
  if start!=fmt['data_offset']+a*2 or end+1!=fmt['data_offset']+b*2 or not 0<=a<b<=fmt['frames']:raise ValueError('invalidcachedframegeometry')
  if not any(a<hi and b>lo for lo,hi in intervals):continue
  data,verified=cached_range(folder,metadata.stem,url,start,end,head['etag'])
  if total!=head['source_bytes']:raise ValueError('cachedsourcebyteschanged')
  chunks.append((a,b,data));audit.append(verified)
  with lock:progress['ranges']+=1
 available=union([(a,b) for a,b,_ in chunks]);holes=[]
 for a,b in intervals:
  cursor=a
  for lo,hi in available:
   if hi<=cursor:continue
   if lo>=b:break
   if lo>cursor:holes.append([cursor,min(lo,b)])
   cursor=max(cursor,hi)
   if cursor>=b:break
  if cursor<b:holes.append([cursor,b])
 for a,b in holes:
  while a<b:
   stop=min(b,a+CAP//2);start=fmt['data_offset']+a*2;end=fmt['data_offset']+stop*2-1
   data,info=cached_range(folder,f'pcm-{a}-{stop}',url,start,end,head['etag'])
   if info['source_bytes']!=head['source_bytes']:raise ValueError('sourcebyteschanged')
   chunks.append((a,stop,data));audit.append(info)
   with lock:progress['ranges']+=1
   a=stop
 chunks.sort(key=lambda c:(c[0],c[1]))
 exported=[]
 for sample in samples:
  windows=[]
  for index,(a,b) in enumerate(sample['source_windows_frames']):
   parts=[];cursor=a
   for lo,hi,data in chunks:
    if hi<=cursor:continue
    if lo>cursor:break
    stop=min(b,hi);parts.append(data[(cursor-lo)*2:(stop-lo)*2]);cursor=stop
    if cursor==b:break
   if cursor!=b:raise ValueError('localrangecoveragegap')
   pcm=b''.join(parts)
   if len(pcm)!=(b-a)*2 or not RATE<=b-a<=5*RATE:raise ValueError('windowgeometrychanged')
   safe=sample['id'].replace('|','__')
   if not re.fullmatch(r'[A-Za-z0-9_-]+',safe):raise ValueError('badSampleID')
   path=output/'windows'/safe/f'{index:03d}-{a}-{b}.wav';wav=make_wav(pcm)
   if path.exists():
    if path.read_bytes()!=wav:raise ValueError('existingwindowchanged')
   else:store(path,wav)
   windows.append({'path':str(path),'sha256':sha(wav),'source_url':url,'source_start_frame':a,'source_end_frame':b,'source_sha256':None,'source_etag':head['etag'],'sample_rate':RATE})
  exported.append({'id':sample['id'],'person_id':sample['person_id'],'meeting_id':mid,'role':sample['role'],'windows':windows})
 result={'meeting_id':mid,'source_url':url,'source_header':head,'format':fmt,'ranges':audit,'samples':exported}
 dump(output/'sources'/(mid+'.json'),result)
 with lock:progress['sources_complete']+=1;completed=progress['sources_complete']
 if completed%5==0:log('progress',meeting_id=mid)
 return result

def main():
 global NETWORK_BUDGET
 parser=argparse.ArgumentParser();parser.add_argument('--partition',type=pathlib.Path,required=True);parser.add_argument('--expected-sha256',required=True);parser.add_argument('--output',type=pathlib.Path,required=True);parser.add_argument('--reuse-ranges',type=pathlib.Path);parser.add_argument('--max-new-bytes',type=int,default=310*1024*1024);args=parser.parse_args()
 if not 0<args.max_new_bytes<=310*1024*1024:raise ValueError('newbytebudgetmustbein1..310MiB')
 NETWORK_BUDGET=args.max_new_bytes
 if args.partition.stat().st_size>16*1024*1024:raise ValueError('partitionexceeds16MiB')
 raw=args.partition.read_bytes()
 if sha(raw)!=args.expected_sha256:raise ValueError('frozenpartitionpinmismatch')
 plan=json.loads(raw);out=args.output.resolve()
 out.mkdir(parents=True,exist_ok=True)
 if (out/'COMPLETE.json').exists():raise ValueError('acquisitionalreadycomplete_verifyorexportexistingdata')
 task={'protocol_id':plan['protocol_id'],'partition_sha256':sha(raw)}
 task_path=out/'TASK.json'
 if task_path.exists() and json.loads(task_path.read_text())!=task:raise ValueError('outputbelongstoanotherfrozenprotocol')
 dump(task_path,task)
 if args.reuse_ranges:
  old=args.reuse_ranges.resolve();new=out/'ranges'
  if not old.is_dir() or not any(old.glob('*/header-0-65535.json')):raise ValueError('reusecachemissing')
  if old==new.resolve():raise ValueError('reusecachemustbedistinctdirectory')
  for source in old.glob('*/'):
   if not source.is_dir() or not re.fullmatch(r'[A-Z]{2}[0-9]{4}[a-d]',source.name):continue
   dest=new/source.name;dest.mkdir(parents=True,exist_ok=True)
   for entry in source.iterdir():
    if entry.is_symlink() or not entry.is_file() or not re.fullmatch(r'(header-0-65535|pcm-[0-9]+-[0-9]+)\.(json|bin)',entry.name):continue
    target=dest/entry.name
    if not target.exists():
     try:os.link(entry,target)
     except OSError:shutil.copyfile(entry,target)
 bymeeting={}
 for p in plan['partitions']:
  for sample in p['samples']:bymeeting.setdefault(sample['meeting_id'],[]).append(sample)
 expected_pcm=sum((b-a)*2 for samples in bymeeting.values() for a,b in coalesce([r for s in samples for r in s['source_windows_frames']]))
 if expected_pcm+len(bymeeting)*HEADER>310*1024*1024:raise ValueError('planneddataexceedsapprovedbudget')
 log('started',protocol_id=plan['protocol_id'],partition_sha256=sha(raw),source_count=len(bymeeting),planned_pcm_bytes=expected_pcm,planned_headers_bytes=len(bymeeting)*HEADER)
 results={};failures=[]
 with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
  jobs={pool.submit(source_job,mid,samples,out):mid for mid,samples in sorted(bymeeting.items())}
  for future in concurrent.futures.as_completed(jobs):
   mid=jobs[future]
   try:results[mid]=future.result()
   except Exception as error:
    failures.append({'meeting_id':mid,'error_type':type(error).__name__,'error':str(error)[:400]})
    with lock:progress['sources_failed']+=1
    log('source_failed',meeting_id=mid,error_type=type(error).__name__,error=str(error)[:180])
 dump(out/'acquisition-status.json',{'protocol_id':plan['protocol_id'],'partition_sha256':sha(raw),'completed':sorted(results),'failures':failures,'progress':progress})
 if failures:log('partial',failures=failures);raise SystemExit(1)
 parts=[]
 for part in plan['partitions']:
  sample_ids={s['id'] for s in part['samples']};samples=[s for r in results.values() for s in r['samples'] if s['id'] in sample_ids]
  def rows(role):return [{k:v for k,v in s.items() if k!='role'} for s in sorted(samples,key=lambda s:s['id']) if s['role']==role]
  parts.append({'id':part['id'],'gallery_people':part['gallery_people'],'enrollments':rows('enrollment'),'probes':rows('probe')})
 manifest={'schema_version':1,'protocol_id':plan['protocol_id'],'channel_type':'in_person','license_url':plan['license_url'],'annotation_sha256':plan['annotation_archive_sha256'],'partition_sha256':sha(raw),'model_dir':'','expected_embedding_contract_sha256':None,'partitions':parts}
 dump(out/'voice-eval-manifest-v1.json',manifest)
 index={'protocol_id':plan['protocol_id'],'partition_sha256':sha(raw),'manifest_sha256':sha((out/'voice-eval-manifest-v1.json').read_bytes()),'source_count':len(results),'sources':[{ 'meeting_id':mid,'manifest_sha256':sha((out/'sources'/(mid+'.json')).read_bytes())} for mid in sorted(results)],'progress':progress,'full_source_hashes':'notclaimed; source_sha256null except independentlycachedfullfilemaybepinnedseparately','biometric_vectors_saved':False}
 dump(out/'COMPLETE.json',index);log('complete',manifest_sha256=index['manifest_sha256'])
if __name__=='__main__':main()
