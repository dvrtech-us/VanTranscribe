#!/usr/bin/env python3
"""Verify that v3 fills only uncovered source bytes, never redownloading cache."""
import hashlib,importlib.util,io,json,pathlib,struct,tempfile,unittest,unittest.mock,wave
root=pathlib.Path(__file__).resolve().parents[1];spec=importlib.util.spec_from_file_location('acquire_v3',root/'acquire_ami.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
url='https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/ES2999a/audio/ES2999a.Array1-01.wav';mid='ES2999a';etag='"fixed"'
pcm=b''.join(struct.pack('<h',(i%30000)-15000) for i in range(80000));buf=io.BytesIO()
with wave.open(buf,'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(pcm)
full=buf.getvalue();calls=[]
def info(start,end):
 data=full[start:end+1]
 return {'source_url':url,'status':206,'content_range':f'bytes {start}-{end}/{len(full)}','source_bytes':len(full),'etag':etag,'last_modified':None,'range_sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}
class CacheReuse(unittest.TestCase):
 def test_only_missing_bytes_are_fetched(self):
  with tempfile.TemporaryDirectory() as folder:
   out=pathlib.Path(folder);cache=out/'ranges'/mid;cache.mkdir(parents=True)
   for name,start,end in [('header-0-65535',0,65535),('pcm-0-16000',44,44+32000-1),('pcm-32000-48000',44+64000,44+96000-1)]:
    (cache/(name+'.bin')).write_bytes(full[start:end+1]);(cache/(name+'.json')).write_text(json.dumps(info(start,end)))
   def fetch(wanted,start,end,wanted_etag):
    assert wanted==url and wanted_etag==etag;calls.append((start,end));return full[start:end+1],info(start,end)
   sample={'id':'calibration|enroll|fixture|ES2999a','person_id':'fixture','meeting_id':mid,'role':'enrollment','source_url':url,'source_windows_frames':[[0,48000]]}
   with unittest.mock.patch.object(a,'request',side_effect=fetch):result=a.source_job(mid,[sample],out)
   assert calls==[(44+32000,44+64000-1)],calls
   win=result['samples'][0]['windows'][0]
   with wave.open(win['path'],'rb') as w:assert w.readframes(w.getnframes())==pcm[:96000]
   assert len(result['ranges'])==3

if __name__=='__main__':unittest.main()
