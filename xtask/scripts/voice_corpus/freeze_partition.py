#!/usr/bin/env python3
"""Freeze labels/source intervals before any model score. Cache-only."""
if not __debug__: raise SystemExit('Run without Python optimization; verification checks are mandatory.')
import argparse,hashlib,json,pathlib
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--plan',type=pathlib.Path,required=True)
parser.add_argument('--output',type=pathlib.Path,required=True)
parser.add_argument('--parent-partition',type=pathlib.Path)
parser.add_argument('--availability-journal',type=pathlib.Path)
args=parser.parse_args()
if args.plan.stat().st_size>16*1024*1024:raise ValueError('planexceeds16MiB')
raw=args.plan.read_bytes();plan=json.loads(raw)
if plan['protocol_id'] not in ['vantranscribe-ami-open-set-in-person-v2','vantranscribe-ami-open-set-in-person-v3']:raise ValueError('unknownprotocol')
if plan['protocol_id'].endswith('-v3') and (not args.parent_partition or not args.availability_journal):raise ValueError('v3requiresparentandavailabilityjournal')

parts=[]
for name,p in plan['partition'].items():
 key='held_out' if name=='heldout' else name
 samples=[]
 for row in plan['samples']:
  if row['phase']!=name:continue
  samples.append({'id':row['sample_id'].replace('heldout|','held_out|',1),'person_id':row['participant_id'],'meeting_id':row['meeting_id'],'role':row['role'],'source_url':row['source_url'],'source_channel':'Array1-01','annotation_agent':row['agent'],'word_annotation':row['word_annotation'],'source_windows_frames':row['selected_spans'],'selected_frames':row['selected_frames']})
 parts.append({'id':key,'project_ids':p['project_ids'],'subject_ids':p['subject_ids'],'gallery_people':p['gallery_person_ids'],'unknown_people':p['unknown_person_ids'],'samples':samples})
protocol={'schema_version':1,'protocol_id':plan['protocol_id'],'status':'frozen_before_audio_acquisition_and_any_model_scores','source_channel':'Array1-01','channel_type':'in_person','sample_rate_expected':16000,'source_native_format_must_verify':'PCM16LE_mono_16000Hz','license_url':'https://creativecommons.org/licenses/by/4.0/','license':'CC-BY-4.0','annotation_archive_sha256':plan['annotations']['sha256'],'meetings_xml_sha256':plan['annotations']['meetings_xml_sha256'],'metadata_plan_sha256':hashlib.sha256(raw).hexdigest(),'selection':{'seed':'vantranscribe-ami-open-set-in-person-v2','site_calibration_project_counts':{'ES':7,'IS':5,'TS':5},'project_sort':'sha256(v2seed|partition|project_id)ascending bysite; ifES2002notinfirst7ES, swaplastselectedESwithES2002; remaining projects held_out','gallery_sort':'sha256(seed|phase|gallery|person_id)ascending among eligiblea-enrollment subjects; phase literal heldout before exportheld_out','gallery_size':50,'enrollment_min_frames':320000,'probe_min_frames':240000,'minimum_distinct_probe_meetings_per_person':2,'enrollment_meeting_suffix':'a','probe_meeting_suffixes':['b','c','d'],'clean_rule':plan['mask_rule'],'span_min_frames':16000,'span_max_frames':80000,'take_rule':'source_order earliestclean spans toquota; lastspan muststill>=1s, mayovershootquota; no merge acrossgaps'},'embedding_recipe':plan['embedding_recipe'],'limits':['scenario a/b/c/d are separate recordings of same-day stagedproject, not independentnaturalmeeting sessions','onlyin_personArray1-01 channel, notremotephone/wideband','referenceoracle identityandword masks bypass productdiarization; end-to-endqualityseparate','officialstandardpartitionnotusedbecauseofficialdev/test20subjects eachcannotform50gallery','no modelscoreorbiometricvectorwasreadorusedinpartitionselection','ES2002alreadyusedinpriorDERmodeldevelopmentisreservedincalibrationonly;v1isretainedaspreacquisitionauditandnotused'],'partitions':parts}
if protocol['protocol_id'].endswith('-v3'):
 protocol['status']='frozen_after_input_availability_audit_before_any_model_scores'
if args.parent_partition:
 protocol['predecessor_partition_sha256']=hashlib.sha256(args.parent_partition.read_bytes()).hexdigest()
 if not args.availability_journal:raise ValueError('availabilityjournalrequiredforv3')
 protocol['availability_audit_sha256']=hashlib.sha256(args.availability_journal.read_bytes()).hexdigest()
 protocol['selection']['project_sort']='keepv2projectphaseassignments; excludeWHOLEprojectifanyselectedrequiredsourcefailedfinalv2availability/nativegeometryaudit; reuseremainingphasev2personhashgalleryranking'
 protocol['selection']['site_calibration_project_counts']={site:sum(p.startswith(site) for p in plan['partition']['calibration']['project_ids']) for site in ['ES','IS','TS']}
p=args.output
p.parent.mkdir(parents=True,exist_ok=True)
with p.open('x') as f:json.dump(protocol,f,indent=2,sort_keys=True);f.write('\n')
print('partition_sha256',hashlib.sha256(p.read_bytes()).hexdigest(),'bytes',p.stat().st_size)
