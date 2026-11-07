"""PCM sanity is data integrity, without voice-quality thresholds or a model."""
import hashlib,json,pathlib,struct,subprocess,sys,tempfile,unittest,wave
SCRIPT=pathlib.Path(__file__).resolve().parents[1]/'audit_pcm.py'
class PCMChecks(unittest.TestCase):
 def test_varied_input_passes_and_zero_dc_or_duplicate_input_stops(self):
  for mode in ['healthy','zero','constant','duplicate']:
   with self.subTest(mode=mode),tempfile.TemporaryDirectory() as folder:
    root=pathlib.Path(folder);windows=[];first=[i%100-50 for i in range(16000)]
    for index in range(2):
     values=first if index==0 or mode=='duplicate' else [i%99-49 for i in range(16000)]
     if index==0 and mode=='zero':values=[0]*16000
     if index==0 and mode=='constant':values=[777]*16000
     path=root/f'{index}.wav'
     with wave.open(str(path),'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(struct.pack('<16000h',*values))
     windows.append({'path':path.name,'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'source_url':'https://groups.inf.ed.ac.uk/ami/fictional.wav','source_start_frame':index*16000,'source_end_frame':(index+1)*16000})
    manifest={'protocol_id':'fictional-pcm-test','partitions':[{'id':'calibration','enrollments':[{'id':'fixture','person_id':'fixture-person','meeting_id':'ES2999a','windows':windows}],'probes':[]}]}
    path=root/'manifest.json';path.write_text(json.dumps(manifest));out=root/'audit.json'
    result=subprocess.run([sys.executable,str(SCRIPT),'--manifest',str(path),'--output',str(out)],capture_output=True,text=True)
    self.assertEqual(result.returncode,0 if mode=='healthy' else 1,result.stderr)
    audit=json.loads(out.read_text());self.assertEqual(audit['allzero_windows'],1 if mode=='zero' else 0);self.assertEqual(audit['constant_pcm_windows'],1 if mode in ['zero','constant'] else 0);self.assertEqual(audit['duplicate_window_sha256_groups'],1 if mode=='duplicate' else 0)
    self.assertFalse(audit['rms_or_amplitude_threshold_used']);self.assertFalse(audit['biometric_vectors_saved'])
if __name__=='__main__':unittest.main()
