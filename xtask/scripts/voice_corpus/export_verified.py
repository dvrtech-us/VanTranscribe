#!/usr/bin/env python3
"""Bind acquired public windows exactly to frozen labels/frames. No vectors."""
if not __debug__: raise SystemExit('Run without Python optimization; verification checks are mandatory.')
import argparse,collections,hashlib,json,pathlib,shutil,wave
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--acquisition',type=pathlib.Path,required=True)
parser.add_argument('--partition',type=pathlib.Path,required=True)
parser.add_argument('--partition-sha256',required=True)
parser.add_argument('--annotations',type=pathlib.Path,required=True)
parser.add_argument('--annotation-sha256',default='b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d')
parser.add_argument('--embedding-contract-sha256',required=True)
parser.add_argument('--full-control',type=pathlib.Path)
parser.add_argument('--full-control-sha256')
parser.add_argument('--output-name',default='voice-eval-template.json')
args=parser.parse_args()
OUT=args.acquisition.resolve();PARTITION_SHA=args.partition_sha256;ANNOTATION_SHA=args.annotation_sha256
if pathlib.Path(args.output_name).name!=args.output_name:raise ValueError('outputnameisnotafilename')

def sha(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  for block in iter(lambda:f.read(262144),b''):h.update(block)
 return h.hexdigest()
def write_new(path,obj):
 with path.open('x') as f:json.dump(obj,f,indent=2,sort_keys=True);f.write('\n')
if args.partition.stat().st_size>16*1024*1024 or args.annotations.stat().st_size>64*1024*1024:raise ValueError('provenanceinputexceedssizebound')
complete=json.loads((OUT/'COMPLETE.json').read_text());assert complete['partition_sha256']==PARTITION_SHA
partpath=args.partition;assert sha(partpath)==PARTITION_SHA;part=json.loads(partpath.read_text())
archive=args.annotations;assert sha(archive)==ANNOTATION_SHA
rawpath=OUT/'voice-eval-manifest-v1.json';assert sha(rawpath)==complete['manifest_sha256'];manifest=json.loads(rawpath.read_text())
source_pins={row['meeting_id']:row['manifest_sha256'] for row in complete['sources']}
for mid,pin in source_pins.items():
 src=OUT/'sources'/(mid+'.json');assert sha(src)==pin
 record=json.loads(src.read_text());head=record['source_header'];fmt=record['format']
 assert head['etag'] and head['status']==206
 assert fmt['sample_rate']==16000 and fmt['channels']==1 and fmt['bits_per_sample']==16
 for i in record['ranges']:
  assert i['etag']==head['etag'] and i['source_bytes']==head['source_bytes'] and i['status']==206
  # Locate the exact acquisition cache by its verified byte range geometry.
  first,last=map(int,i['content_range'].split()[1].split('/')[0].split('-'))
  assert (first-fmt['data_offset'])%2==0 and (last+1-fmt['data_offset'])%2==0
  a=(first-fmt['data_offset'])//2;b=(last+1-fmt['data_offset'])//2
  cache=OUT/'ranges'/mid/f'pcm-{a}-{b}.bin';assert sha(cache)==i['range_sha256'] and cache.stat().st_size==i['bytes']
metadata={};global_ranges=collections.defaultdict(list);window_count=0;window_bytes=0
assert len(part['partitions'])==len(manifest['partitions']), 'partition count differs from frozen selection'
for frozen_part,export_part in zip(part['partitions'],manifest['partitions']):
 assert frozen_part['id']==export_part['id'] and frozen_part['gallery_people']==export_part['gallery_people']
 frozen={x['id']:x for x in frozen_part['samples']};seen=set()
 for role,key in [('enrollment','enrollments'),('probe','probes')]:
  for sample in export_part[key]:
   row=frozen[sample['id']];assert row['role']==role and row['person_id']==sample['person_id'] and row['meeting_id']==sample['meeting_id']
   project=[p for p in frozen_part['project_ids'] if row['meeting_id'].startswith(p)];assert len(project)==1
   sample['project_id']=project[0];assert sample['id'] not in seen;seen.add(sample['id'])
   assert len(sample['windows'])==len(row['source_windows_frames'])
   duration=0
   for win,(a,b) in zip(sample['windows'],row['source_windows_frames']):
    assert win['source_start_frame']==a and win['source_end_frame']==b and win['source_url']==row['source_url']
    assert 16000<=b-a<=80000 and win['sample_rate']==16000
    p=pathlib.Path(win['path']);assert OUT/'windows' in p.resolve().parents and not p.is_symlink()
    assert sha(p)==win['sha256'] and p.stat().st_size==44+(b-a)*2
    with wave.open(str(p),'rb') as w:
     assert (w.getnchannels(),w.getsampwidth(),w.getframerate(),w.getnframes(),w.getcomptype())==(1,2,16000,b-a,'NONE')
    source=json.loads((OUT/'sources'/(row['meeting_id']+'.json')).read_text());assert win['source_etag']==source['source_header']['etag']
    global_ranges[win['source_url']].append((a,b,sample['id']));window_count+=1;window_bytes+=p.stat().st_size;duration+=b-a
   assert duration>=320000 if role=='enrollment' else duration>=240000
 assert seen==set(frozen)
for url,ranges in global_ranges.items():
 ranges.sort();assert all(ranges[i][1]<=ranges[i+1][0] for i in range(len(ranges)-1)),url
# Independently compare downloaded ES2002a windows to the earlier full official
# source cache, proving byte-offset/crop geometry on a known full-source control.
full=args.full_control;checked=0
if full is not None:
 if not args.full_control_sha256:raise ValueError('fullcontrolrequiresdeclaredSHA')
 fullsha=sha(full);assert fullsha==args.full_control_sha256
 with wave.open(str(full),'rb') as original:
  for p in manifest['partitions']:
   for s in p['enrollments']+p['probes']:
    if s['meeting_id']!='ES2002a':continue
    for win in s['windows']:
     original.setpos(win['source_start_frame']);pcm=original.readframes(win['source_end_frame']-win['source_start_frame'])
     with wave.open(win['path'],'rb') as clipped:assert clipped.readframes(clipped.getnframes())==pcm
     win['source_sha256']=fullsha;checked+=1
prov=OUT/'provenance';prov.mkdir(exist_ok=True)
for src,name,pin in [(archive,'ami_public_manual_1.6.2.zip',ANNOTATION_SHA),(partpath,'partition.json',PARTITION_SHA)]:
 dest=prov/name
 if not dest.exists():shutil.copyfile(src,dest)
 assert sha(dest)==pin
manifest['expected_embedding_contract_sha256']=args.embedding_contract_sha256
manifest['annotation_path']='provenance/ami_public_manual_1.6.2.zip';manifest['partition_path']='provenance/partition.json'
for partition in manifest['partitions']:
 for sample in partition['enrollments']+partition['probes']:
  for window in sample['windows']:
   window['path']=pathlib.Path(window['path']).resolve().relative_to(OUT).as_posix()
path=OUT/args.output_name;write_new(path,manifest)
verified={'schema_version':1,'protocol_id':part['protocol_id'],'partition_sha256':PARTITION_SHA,'annotation_sha256':ANNOTATION_SHA,'acquisition_complete_sha256':sha(OUT/'COMPLETE.json'),'template_path':str(path),'template_sha256':sha(path),'source_count':len(source_pins),'window_count':window_count,'window_bytes':window_bytes,'samples':sum(len(x['enrollments'])+len(x['probes']) for x in manifest['partitions']),'global_origin_overlap':False,'exact_full_source_control_ES2002a_windows':checked,'biometric_vectors_saved':False,'model_fields':'rawcontractpinnedbeforeinference; rootmustpinspecificmodeldirectorybeforeinference'}
write_new(OUT/'EXPORT-VERIFIED.json',verified);print(json.dumps(verified,indent=2))
