#!/usr/bin/env python3
"""Input-only native PCM sanity; no RMS threshold, model, vectors or exclusions."""
if not __debug__: raise SystemExit('Run without Python optimization; verification checks are mandatory.')
import argparse,array,collections,datetime,hashlib,json,pathlib,sys,wave
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--manifest',type=pathlib.Path,required=True)
parser.add_argument('--output',type=pathlib.Path,required=True)
args=parser.parse_args()
if args.manifest.stat().st_size>16*1024*1024:raise ValueError('manifestexceeds16MiB')
raw=args.manifest.read_bytes();manifest=json.loads(raw);root=args.manifest.resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
phases=[];descriptors=[];problems=[];allzero_samples=0;constant_windows=0;hash_origins=collections.defaultdict(list)
for part in manifest['partitions']:
 totals={'id':part['id'],'windows':0,'samples':0,'frames':0,'nonzero_frames':0,'allzero_windows':0,'allzero_samples':0,'peak_native_pcm16':0}
 for role,key in [('enrollment','enrollments'),('probe','probes')]:
  for sample in part[key]:
   totals['samples']+=1;sample_nonzero=0
   for window in sample['windows']:
    relative=pathlib.Path(window['path'])
    if relative.is_absolute() or '..' in relative.parts:raise ValueError('windowpathmustbecontainedrelativepath')
    path=(root/relative).resolve()
    if root not in path.parents:raise ValueError('windowsymlinkescapesmanifestroot')
    if not 32044<=path.stat().st_size<=160044:raise ValueError('windowexceeds1..5sPCM16bound')
    wav=path.read_bytes()
    if sha(wav)!=window['sha256']:raise ValueError('windowhashchanged')
    with wave.open(str(path),'rb') as source:
     if (source.getnchannels(),source.getsampwidth(),source.getframerate(),source.getcomptype())!=(1,2,16000,'NONE'):raise ValueError('unexpectednativeformat')
     n=source.getnframes()
     if n!=window['source_end_frame']-window['source_start_frame']:raise ValueError('originframegeometrychanged')
     pcm=source.readframes(n)
    values=array.array('h');values.frombytes(pcm)
    if sys.byteorder!='little':values.byteswap()
    if len(values)!=n or not n:raise ValueError('emptyortruncatedPCM')
    nonzero=n-values.count(0);lo=min(values);hi=max(values);peak=max(abs(lo),abs(hi))
    totals['windows']+=1;totals['frames']+=n;totals['nonzero_frames']+=nonzero;sample_nonzero+=nonzero;totals['peak_native_pcm16']=max(totals['peak_native_pcm16'],peak)
    if nonzero==0:
     totals['allzero_windows']+=1;problems.append({'partition':part['id'],'sample_id':sample['id'],'path':window['path'],'reason':'allzero_native_PCM_window'})
    if lo==hi:
     constant_windows+=1;problems.append({'partition':part['id'],'sample_id':sample['id'],'path':window['path'],'reason':'constant_native_PCM_window'})
    hash_origins[window['sha256']].append({'partition':part['id'],'role':role,'sample_id':sample['id'],'path':window['path'],'source_url':window['source_url'],'source_start_frame':window['source_start_frame'],'source_end_frame':window['source_end_frame']})
    descriptors.append({'partition':part['id'],'role':role,'sample_id':sample['id'],'person_id':sample['person_id'],'meeting_id':sample['meeting_id'],'path':window['path'],'wav_sha256':window['sha256'],'source_url':window['source_url'],'source_start_frame':window['source_start_frame'],'source_end_frame':window['source_end_frame']})
   if sample_nonzero==0:totals['allzero_samples']+=1;allzero_samples+=1
 phases.append(totals)
duplicate_groups=[]
for digest,origins in hash_origins.items():
 distinct={(row['source_url'],row['source_start_frame'],row['source_end_frame']) for row in origins}
 if len(distinct)>1:
  duplicate_groups.append({'wav_sha256':digest,'origins':origins});problems.append({'reason':'duplicate_window_across_distinct_origins','wav_sha256':digest,'occurrences':len(origins)})
window_descriptor_bytes=json.dumps(sorted(descriptors,key=lambda d:(d['partition'],d['sample_id'],d['source_start_frame'])),sort_keys=True,separators=(',',':')).encode()
report={'schema_version':1,'protocol_id':manifest['protocol_id'],'manifest_sha256':sha(raw),'input_window_descriptor_sha256':sha(window_descriptor_bytes),'recorded_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'scope':'nativePCMinputsanityonly;notquality/identification/modelscore','rms_or_amplitude_threshold_used':False,'biometric_vectors_saved':False,'phases':phases,'allzero_windows':sum(p['allzero_windows'] for p in phases),'constant_pcm_windows':constant_windows,'duplicate_window_sha256_groups':len(duplicate_groups),'duplicate_details':duplicate_groups,'allzero_samples':allzero_samples,'problems':problems,'status':'stop_before_model' if problems or allzero_samples else 'passed'}
with args.output.open('x') as f:json.dump(report,f,indent=2,sort_keys=True);f.write('\n')
print(json.dumps({k:v for k,v in report.items() if k!='problems'},indent=2))
if problems or allzero_samples:raise SystemExit(1)
