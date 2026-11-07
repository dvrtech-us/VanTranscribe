#!/usr/bin/env python3
"""No-network regression checks for bounded public-corpus acquisition."""
import contextlib, importlib.util, io, pathlib, tempfile, unittest, unittest.mock, wave
P=pathlib.Path(__file__).resolve().parents[1]/'acquire_ami.py'
spec=importlib.util.spec_from_file_location('acquire',P);a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
URL='https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/ES2999a/audio/ES2999a.Array1-01.wav'
class Response:
 def __init__(self,status=206,headers=None,data=b'abc',url=URL):self.status=status;self.headers={'Content-Range':'bytes 0-2/30','Content-Length':'3','ETag':'"fixed"',**(headers or {})};self.data=data;self.url=url;self.reads=[]
 def __enter__(self):return self
 def __exit__(self,*args):pass
 def geturl(self):return self.url
 def read(self,n):self.reads.append(n);return self.data[:n]
class Checks(unittest.TestCase):
 def attempt(self,r,**kwargs):
  with unittest.mock.patch.object(a,'urlopen',return_value=r),unittest.mock.patch.object(a.time,'sleep'):
   return a.request(URL,0,2,**kwargs)
 def test_200_rejected_without_reading_full_body(self):
  r=Response(status=200,data=b'x'*1000000)
  with self.assertRaises(ValueError):self.attempt(r)
  self.assertEqual(r.reads,[])
 def test_wrong_range_redirect_weaketag_and_encoding_rejected_before_read(self):
  for r in [Response(headers={'Content-Range':'bytes 1-3/30'}),Response(url='https://unrelated.invalid/test.wav'),Response(headers={'ETag':'W/"fixed"'}),Response(headers={'Content-Encoding':'gzip'})]:
   with self.assertRaises(ValueError):self.attempt(r)
   self.assertEqual(r.reads,[])
 def test_changed_etag_cannot_mix_source_revisions(self):
  r=Response()
  with self.assertRaises(ValueError):self.attempt(r,etag='"older"')
  self.assertEqual(r.reads,[])
 def test_payload_is_bounded_and_exact(self):
  r=Response();data,info=self.attempt(r);self.assertEqual(data,b'abc');self.assertEqual(r.reads,[4]);self.assertEqual(info['bytes'],3)
  for payload in (b'ab',b'abcd'):
   with self.assertRaises(ValueError):self.attempt(Response(data=payload))
 def test_cap_rejects_before_network(self):
  with unittest.mock.patch.object(a,'urlopen') as network:
   with self.assertRaises(ValueError):a.request(URL,0,a.CAP)
   network.assert_not_called()
  with unittest.mock.patch.object(a,'NETWORK_BUDGET',a.progress['network_bytes']+2),unittest.mock.patch.object(a,'urlopen') as network:
   with self.assertRaises(ValueError):a.request(URL,0,2)
   network.assert_not_called()
  self.assertEqual(a.network_reserved,0)
 def test_cache_reused_only_with_matching_hash_and_metadata(self):
  with tempfile.TemporaryDirectory() as folder:
   r=Response()
   with unittest.mock.patch.object(a,'urlopen',return_value=r),unittest.mock.patch.object(a.time,'sleep'):
    first=a.cached_range(pathlib.Path(folder),'range',URL,0,2,'"fixed"')
   with unittest.mock.patch.object(a,'request') as network:
    second=a.cached_range(pathlib.Path(folder),'range',URL,0,2,'"fixed"');network.assert_not_called()
   self.assertEqual(first,second)
   (pathlib.Path(folder)/'range.bin').write_bytes(b'xyz')
   with unittest.mock.patch.object(a,'request') as network:
    with self.assertRaises(ValueError):a.cached_range(pathlib.Path(folder),'range',URL,0,2,'"fixed"')
    network.assert_not_called()
 def test_native_geometry_and_data_bounds_fail_closed(self):
  f=io.BytesIO()
  with wave.open(f,'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(16000);w.writeframes(b'\0'*32000)
  data=f.getvalue();self.assertEqual(a.geometry(data,len(data))['frames'],16000)
  with self.assertRaises(ValueError):a.geometry(data,len(data)-1)
  bad=bytearray(data);bad[24:28]=(48000).to_bytes(4,'little')
  with self.assertRaises(ValueError):a.geometry(bytes(bad),len(bad))
 def test_coalescing_bounds_and_origin_support(self):
  spans=[[0,1000000],[1000001,1000010],[2000000,2000100]];ranges=a.coalesce(spans)
  self.assertTrue(all((hi-lo)*2<=a.CAP for lo,hi in ranges))
  for left,right in spans:self.assertEqual(sum(max(0,min(b,right)-max(x,left)) for x,b in ranges),right-left)
 def test_only_exact_official_https_origin_is_allowed(self):
  for url in ['http://groups.inf.ed.ac.uk/ami/file.wav','https://unrelated.invalid/file.wav',URL+'?token=private','https://user:secret@groups.inf.ed.ac.uk/ami/file.wav']:
   with unittest.mock.patch.object(a,'urlopen') as network:
    with self.assertRaises(ValueError):a.request(url,0,2)
    network.assert_not_called()
  with self.assertRaises(ValueError):a.NoRedirect().redirect_request(None,None,None,None,None,None)
if __name__=='__main__':unittest.main()

