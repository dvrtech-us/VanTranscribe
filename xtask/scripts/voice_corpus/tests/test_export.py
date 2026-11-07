"""Exporter integration uses fictional metadata and generated PCM only."""
import hashlib,json,pathlib,struct,subprocess,sys,tempfile,unittest,wave
ROOT=pathlib.Path(__file__).resolve().parents[1]
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def write(path,value):path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value))
class PortableExport(unittest.TestCase):
 def test_verified_template_uses_relative_contained_paths(self):
  with tempfile.TemporaryDirectory() as folder:
   out=pathlib.Path(folder);mid='ES2999a';url=f'https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{mid}/audio/{mid}.Array1-01.wav'
   pcm=b''.join(struct.pack('<h',(i%30000)-15000) for i in range(320000));windows=[];spans=[]
   for index in range(4):
    first=index*80000;last=first+80000;path=out/'windows'/f'{index}.wav';path.parent.mkdir(exist_ok=True)
    with wave.open(str(path),'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(pcm[first*2:last*2])
    spans.append([first,last]);windows.append({'path':str(path),'sha256':digest(path),'source_url':url,'source_start_frame':first,'source_end_frame':last,'source_sha256':None,'source_etag':'"fixture"','sample_rate':16000})
   cached=out/'ranges'/mid/'pcm-0-320000.bin';cached.parent.mkdir(parents=True);cached.write_bytes(pcm)
   source=out/'sources'/f'{mid}.json';write(source,{'source_header':{'etag':'"fixture"','status':206,'source_bytes':640044},'format':{'sample_rate':16000,'channels':1,'bits_per_sample':16,'data_offset':44},'ranges':[{'etag':'"fixture"','source_bytes':640044,'status':206,'content_range':'bytes 44-640043/640044','range_sha256':digest(cached),'bytes':640000}]})
   sample={'id':'calibration|enroll|fixture|ES2999a','person_id':'fixture','meeting_id':mid,'windows':windows}
   raw=out/'voice-eval-manifest-v1.json';write(raw,{'schema_version':1,'protocol_id':'fictional-export-test','channel_type':'in_person','license_url':'https://creativecommons.org/licenses/by/4.0/','model_dir':'','expected_embedding_contract_sha256':None,'partitions':[{'id':'calibration','gallery_people':['fixture'],'enrollments':[sample],'probes':[]}]})
   part=out/'frozen.json';write(part,{'protocol_id':'fictional-export-test','partitions':[{'id':'calibration','project_ids':['ES2999'],'gallery_people':['fixture'],'samples':[{'id':sample['id'],'person_id':'fixture','meeting_id':mid,'role':'enrollment','source_url':url,'source_windows_frames':spans}]}]})
   annotations=out/'fictional-annotations.bin';annotations.write_bytes(b'fictional provenance fixture; no corpus data')
   # Digests are supplied explicitly to this verifier fixture, not to acquisition.
   value=json.loads(raw.read_text());value.update(annotation_sha256=digest(annotations),partition_sha256=digest(part));write(raw,value)
   write(out/'COMPLETE.json',{'partition_sha256':digest(part),'manifest_sha256':digest(raw),'sources':[{'meeting_id':mid,'manifest_sha256':digest(source)}]})
   result=subprocess.run([sys.executable,str(ROOT/'export_verified.py'),'--acquisition',str(out),'--partition',str(part),'--partition-sha256',digest(part),'--annotations',str(annotations),'--annotation-sha256',digest(annotations),'--embedding-contract-sha256','0'*64],capture_output=True,text=True)
   self.assertEqual(result.returncode,0,result.stderr)
   exported=json.loads((out/'voice-eval-template.json').read_text())
   for path in [exported['annotation_path'],exported['partition_path']]+[w['path'] for w in exported['partitions'][0]['enrollments'][0]['windows']]:
    self.assertFalse(pathlib.Path(path).is_absolute());self.assertNotIn('..',pathlib.Path(path).parts);self.assertTrue((out/path).is_file())
   self.assertEqual(exported['partitions'][0]['enrollments'][0]['project_id'],'ES2999')
if __name__=='__main__':unittest.main()
