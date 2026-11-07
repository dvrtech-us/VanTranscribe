#!/usr/bin/env python3
"""Cache-only official AMI identity/speech audit. Never builds biometric vectors."""
if not __debug__: raise SystemExit('Run without Python optimization; verification checks are mandatory.')
import argparse, collections, hashlib, json, pathlib, re, xml.etree.ElementTree as ET, zipfile
from decimal import Decimal, ROUND_CEILING, ROUND_FLOOR
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--annotations',type=pathlib.Path,required=True)
parser.add_argument('--output',type=pathlib.Path,required=True)
parser.add_argument('--protocol',choices=['v2','v3'],default='v2')
parser.add_argument('--parent-partition',type=pathlib.Path)
parser.add_argument('--availability-status',type=pathlib.Path)
args=parser.parse_args()
ROOT=args.output;ARCHIVE=args.annotations
ROOT.mkdir(parents=True,exist_ok=True)
if ARCHIVE.stat().st_size>64*1024*1024:raise ValueError('annotationarchiveexceeds64MiB')
PROTOCOL_ID='vantranscribe-ami-open-set-in-person-'+args.protocol

RATE=16000
SEED='vantranscribe-ami-open-set-in-person-v2'
def digest(b):return hashlib.sha256(b).hexdigest()
def rank(label,value):return digest((SEED+'|'+label+'|'+value).encode())
def union(items):
 out=[]
 for a,b in sorted(items):
  if b<=a:continue
  if out and a<=out[-1][1]:out[-1][1]=max(out[-1][1],b)
  else:out.append([a,b])
 return out

def subtract(target,blocked):
 out=[];j=0
 for a,b in target:
  while j<len(blocked) and blocked[j][1]<=a:j+=1
  left=a;k=j
  while k<len(blocked) and blocked[k][0]<b:
   c,d=blocked[k]
   if c>left:out.append([left,min(c,b)])
   left=max(left,d)
   if left>=b:break
   k+=1
  if left<b:out.append([left,b])
 return out

def frames(value,upper):return int((Decimal(value)*RATE).to_integral_value(rounding=ROUND_CEILING if upper else ROUND_FLOOR))
def usable(items):
 # Preserve source order, no silence insertion or joining across gaps. Split
 # each contiguous clean island into1–5s spans exactly like production enrollment.
 out=[]
 for a,b in items:
  while b-a>=RATE:
   stop=min(b,a+5*RATE);out.append([a,stop]);a=stop
 return out

def take(items,target):
 out=[];remaining=target
 for a,b in items:
  if remaining<=0:break
  stop=min(b,a+remaining)
  # Never add a subsecond inference span just to hit the quota exactly.
  if stop-a<RATE:stop=min(b,a+RATE)
  out.append([a,stop]);remaining-=stop-a
 if remaining>0:return None
 return out

blob=ARCHIVE.read_bytes();assert digest(blob)=='b56e5babb2496b8795deeeda7e71178d7fbc9963f94276cf2a3f4b56ebbc9f9d'
z=zipfile.ZipFile(ARCHIVE)
if sum(info.file_size for info in z.infolist())>1024*1024*1024:raise ValueError('uncompressedarchiveexceeds1GiB')
for info in z.infolist():
 path=pathlib.PurePosixPath(info.filename)
 if path.is_absolute() or '..' in path.parts or '\\' in info.filename:raise ValueError('unsafearchivemember')
def xml(name,limit):
 info=z.getinfo(name)
 if info.file_size>limit:raise ValueError('XMLsizebound')
 data=z.read(name)
 if b'<!DOCTYPE' in data or b'<!ENTITY' in data:raise ValueError('XMLentitiesarenotsupported')
 return data,ET.fromstring(data)
metadata,meetings=xml('corpusResources/meetings.xml',8*1024*1024)

records={};people=collections.defaultdict(list);source_pins={};types=collections.defaultdict(lambda:collections.defaultdict(set))
for m in meetings:
 mid=m.attrib['observation'];assert re.fullmatch(r'[A-Z][SNB][0-9]{4}[a-z]?',mid);cat='scenario' if mid[1]=='S' else 'natural' if mid[1]=='N' else 'other_staged'
 for speaker in m.findall('speaker'):types[cat][speaker.attrib['global_name']].add(mid)
 if cat!='scenario':continue
 speaker_rows={s.attrib['nxt_agent']:s for s in m.findall('speaker')}
 lexical={};activity={}
 for agent,s in speaker_rows.items():
  assert re.fullmatch(r'[A-Z]',agent)
  path=f'words/{mid}.{agent}.words.xml';data,word_root=xml(path,4*1024*1024)
  source_pins[path]=digest(data);lex=[];act=[]
  for w in word_root:
   if w.tag not in ('w','vocalsound') or 'starttime' not in w.attrib or 'endtime' not in w.attrib:continue
   a=frames(w.attrib['starttime'],True);b=frames(w.attrib['endtime'],False)
   if a<0 or b<=a:continue
   act.append([a,b])
   if w.tag=='w' and w.attrib.get('punc')!='true' and ''.join(w.itertext()).strip():lex.append([a,b])
  lexical[agent]=union(lex);activity[agent]=union(act)
 for agent,s in speaker_rows.items():
  others=union([interval for other,items in activity.items() if other!=agent for interval in items])
  clean=subtract(lexical[agent],others);spans=usable(clean);pid=s.attrib['global_name']
  record={'meeting_id':mid,'participant_id':pid,'agent':agent,'native_rate_assumed':RATE,'channel':'Array1-01','source_url':f'https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{mid}/audio/{mid}.Array1-01.wav','clean_lexical_frames':sum(b-a for a,b in clean),'usable_frames':sum(b-a for a,b in spans),'spans':spans,'word_annotation':f'words/{mid}.{agent}.words.xml'}
  records[(mid,pid)]=record;people[pid].append(record)

teams=sorted({r['meeting_id'][:6] for rows in people.values() for r in rows});calibration=[];evaluation=[]
for site,ncal in [('ES',7),('IS',5),('TS',5)]:
 site_teams=sorted((t for t in teams if t.startswith(site)),key=lambda t:rank('partition',t))
 selected=site_teams[:ncal];remaining=site_teams[ncal:]
 if site=='ES' and 'ES2002' not in selected:
  displaced=selected[-1];selected=selected[:-1]+['ES2002'];remaining=[t for t in remaining if t!='ES2002']+[displaced]
 calibration.extend(selected);evaluation.extend(remaining)
if args.protocol=='v3':
 if not args.parent_partition or not args.availability_status:raise ValueError('v3requiresparentpartitionandterminalavailabilitystatus')
 parent_blob=args.parent_partition.read_bytes();parent=json.loads(parent_blob)
 status=json.loads(args.availability_status.read_text())
 if status['partition_sha256']!=digest(parent_blob):raise ValueError('parentavailabilitypinmismatch')
 required_sources={s['meeting_id'] for p in parent['partitions'] for s in p['samples']}
 observed=set(status['completed'])|{f['meeting_id'] for f in status['failures']}
 if observed!=required_sources:raise ValueError('availabilityauditnotcomplete')
 for failure in status['failures']:
  permanent=failure['error_type']=='HTTPError' and any(f'HTTP Error {code}:' in failure['error'] for code in [403,404,410])
  geometry=failure['error_type']=='ValueError' and failure['error'] in ['unexpectedgeometry','notRIFFWAVE','truncatedfmt']
  if not(permanent or geometry):raise ValueError('transientorunclassifiedfailure_requiresseparatereview')
 failed_projects={failure['meeting_id'][:6] for failure in status['failures']}
 calibration=[t for p in parent['partitions'] if p['id']=='calibration' for t in p['project_ids'] if t not in failed_projects]
 evaluation=[t for p in parent['partitions'] if p['id']=='held_out' for t in p['project_ids'] if t not in failed_projects]
 journal={'schema_version':1,'protocol_id':PROTOCOL_ID,'parent_partition_sha256':digest(parent_blob),'terminal_status_sha256':digest(args.availability_status.read_bytes()),'source_failures':status['failures'],'excluded_project_ids':sorted(failed_projects),'rule':'excludeWHOLEprojectwithpermanentHTTP403/404/410ornativeformatfailure;keepv2phaseassignmentsandv2personhashrank;nomodelscoresused'}
 journal_path=ROOT/'availability-exclusions-v3.json'
 with journal_path.open('x') as f:json.dump(journal,f,indent=2,sort_keys=True);f.write('\n')

phases={};selected=[];exclusions=[]
for phase,chosen in [('calibration',calibration),('heldout',evaluation)]:
 candidates={pid:sorted([r for r in rows if r['meeting_id'][:6] in chosen],key=lambda r:r['meeting_id']) for pid,rows in people.items() if any(r['meeting_id'][:6] in chosen for r in rows)}
 eligible={};known=[]
 for pid,rows in candidates.items():
  enroll=next((r for r in rows if r['meeting_id'].endswith('a') and r['usable_frames']>=20*RATE),None)
  probes=[r for r in rows if r['meeting_id'][-1] in 'bcd' and r['usable_frames']>=15*RATE]
  if len(probes)<2:exclusions.append({'phase':phase,'participant_id':pid,'reason':'fewer_than_two_probe_meetings','probe_meetings':[r['meeting_id'] for r in probes]});continue
  eligible[pid]=(enroll,probes)
  if enroll:known.append(pid)
 gallery=sorted(known,key=lambda pid:rank(phase+'|gallery',pid))[:50];assert len(gallery)==50,(phase,len(known))
 unknown=sorted(set(eligible)-set(gallery))
 for pid in gallery:
  r=eligible[pid][0];selected.append({**r,'phase':phase,'role':'enrollment','sample_id':phase+'|enroll|'+pid+'|'+r['meeting_id'],'expected_person_id':pid,'selected_spans':take(r['spans'],20*RATE)})
 for pid in sorted(eligible):
  for r in eligible[pid][1]:selected.append({**r,'phase':phase,'role':'probe','sample_id':phase+'|probe|'+pid+'|'+r['meeting_id'],'expected_person_id':pid if pid in gallery else None,'selected_spans':take(r['spans'],15*RATE)})
 phases[phase]={'project_ids':sorted(chosen),'subject_ids':sorted(candidates),'gallery_person_ids':gallery,'unknown_person_ids':unknown,'candidate_known_subjects':len(known),'probe_trials':sum(len(v[1]) for v in eligible.values()),'unknown_trials':sum(len(eligible[p][1]) for p in unknown)}
assert not set(phases['calibration']['subject_ids'])&set(phases['heldout']['subject_ids'])
assert not set(calibration)&set(evaluation)
for r in selected:
 r.pop('spans');assert r['selected_spans'];r['selected_frames']=sum(b-a for a,b in r['selected_spans'])
byfile=collections.defaultdict(list)
for r in selected:byfile[r['source_url']].extend(r['selected_spans'])
requests={}
for gap_seconds in (0,1,2,5):
 ranges={}
 for url,items in byfile.items():
  joined=[]
  for a,b in union(items):
   if joined and a-joined[-1][1]<=gap_seconds*RATE:joined[-1][1]=b
   else:joined.append([a,b])
  ranges[url]=joined
 requests[str(gap_seconds)]={'requests':sum(map(len,ranges.values())),'pcm_bytes':sum((b-a)*2 for rows in ranges.values() for a,b in rows),'max_single_request_bytes':max((b-a)*2 for rows in ranges.values() for a,b in rows)}
summary={'protocol_id':PROTOCOL_ID,'annotation_archive_sha256':digest(blob),'meetings_xml_sha256':digest(metadata),'source_metadata_counts':{k:{'meetings':len({m for q in v.values() for m in q}),'subjects':len(v),'recurring_subjects':sum(len(q)>1 for q in v.values()),'subjects_across_project_prefixes':sum(len({m[:6] for m in q})>1 for q in v.values())} for k,v in types.items()},'phases':{k:{a:(len(b) if isinstance(b,list) else b) for a,b in v.items()} for k,v in phases.items()},'samples':len(selected),'source_meetings':len(byfile),'selected_frames':sum(r['selected_frames'] for r in selected),'selected_pcm_bytes':sum(r['selected_frames']*2 for r in selected),'range_coalescing_estimates':requests,'exclusions':exclusions}
manifest={'protocol_id':PROTOCOL_ID,'status':'metadata_plan_not_model_scored','rate_expected':RATE,'channel_type':'in_person','annotations':{'path':str(ARCHIVE),'sha256':digest(blob),'meetings_xml_sha256':digest(metadata),'word_file_sha256':source_pins},'partition':phases,'samples':selected,'exclusions':exclusions,'mask_rule':'positive-duration nonpunctuation words; union touching only; subtract other participants positive-duration words and vocal sounds; keep contiguous1–5s spans without gap filling; no word text is stored','embedding_recipe':'L2 normalized512d span vectors; f64 duration-weighted mean thenL2, productionVoiceEmbedder; inRAMonly','license':'CC-BY-4.0','attribution':'AMI Meeting Corpus, AMI Consortium; University of Edinburgh/CSTR and participating institutions. https://groups.inf.ed.ac.uk/ami/corpus/','source_urls':['https://groups.inf.ed.ac.uk/ami/AMICorpusAnnotations/ami_public_manual_1.6.2.zip','https://groups.inf.ed.ac.uk/ami/download/','https://groups.inf.ed.ac.uk/ami/corpus/participantids.shtml','https://groups.inf.ed.ac.uk/ami/corpus/meetingids.shtml']}
for name,data in [(f'metadata-summary-{args.protocol}.json',summary),(f'metadata-plan-{args.protocol}.json',manifest)]:
 path=ROOT/name
 with path.open('x') as f:json.dump(data,f,indent=2,sort_keys=True);f.write('\n')
 print(name,'sha256',digest(path.read_bytes()),'bytes',path.stat().st_size)
(ROOT/'meetings.xml').write_bytes(metadata);(ROOT/'AMI-LICENCE.txt').write_bytes(z.read('LICENCE.txt'))
print(json.dumps(summary,indent=2))
