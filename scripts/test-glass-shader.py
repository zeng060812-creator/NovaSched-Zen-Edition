"""Compare the WebGL lens against APK AGSL on a software EGL device. Requires Mesa EGL."""
import ctypes as c, ctypes.util, re, os
from pathlib import Path
os.environ['EGL_PLATFORM']='surfaceless'
egl_name=ctypes.util.find_library('EGL')
if not egl_name:print('UNAVAILABLE: Mesa EGL library not found on this host');raise SystemExit(2)
e=c.CDLL(egl_name)
e.eglGetProcAddress.argtypes=[c.c_char_p];e.eglGetProcAddress.restype=c.c_void_p
getdisplay=c.CFUNCTYPE(c.c_void_p,c.c_uint,c.c_void_p,c.POINTER(c.c_int))(e.eglGetProcAddress(b'eglGetPlatformDisplayEXT'))
display=getdisplay(0x31dd,None,None)
e.eglInitialize.argtypes=[c.c_void_p,c.POINTER(c.c_int),c.POINTER(c.c_int)];e.eglInitialize.restype=c.c_uint
major,minor=c.c_int(),c.c_int()
if not e.eglInitialize(display,c.byref(major),c.byref(minor)):print('UNAVAILABLE: EGL display could not initialize');raise SystemExit(2)
e.eglBindAPI.argtypes=[c.c_uint];e.eglBindAPI(0x30a0)
attrs=(c.c_int*13)(0x3033,1,0x3040,4,0x3024,8,0x3023,8,0x3022,8,0x3021,8,0x3038)
config=c.c_void_p();count=c.c_int()
e.eglChooseConfig.argtypes=[c.c_void_p,c.POINTER(c.c_int),c.POINTER(c.c_void_p),c.c_int,c.POINTER(c.c_int)];e.eglChooseConfig.restype=c.c_uint
assert e.eglChooseConfig(display,attrs,c.byref(config),1,c.byref(count)) and count.value
BUFFER_W,BUFFER_H=192,128
surfaceattrs=(c.c_int*5)(0x3057,BUFFER_W,0x3056,BUFFER_H,0x3038)
e.eglCreatePbufferSurface.argtypes=[c.c_void_p,c.c_void_p,c.POINTER(c.c_int)];e.eglCreatePbufferSurface.restype=c.c_void_p
surface=e.eglCreatePbufferSurface(display,config,surfaceattrs)
contextattrs=(c.c_int*3)(0x3098,2,0x3038)
e.eglCreateContext.argtypes=[c.c_void_p,c.c_void_p,c.c_void_p,c.POINTER(c.c_int)];e.eglCreateContext.restype=c.c_void_p
context=e.eglCreateContext(display,config,None,contextattrs)
e.eglMakeCurrent.argtypes=[c.c_void_p,c.c_void_p,c.c_void_p,c.c_void_p];e.eglMakeCurrent.restype=c.c_uint
assert e.eglMakeCurrent(display,surface,surface,context)
def gl(name,restype,args):
 address=e.eglGetProcAddress(name.encode());assert address,name
 return c.CFUNCTYPE(restype,*args)(address)
getstring=gl('glGetString',c.c_char_p,[c.c_uint]);print('GPU',getstring(0x1f01).decode(),'API',getstring(0x1f02).decode())
createshader=gl('glCreateShader',c.c_uint,[c.c_uint]);source=gl('glShaderSource',None,[c.c_uint,c.c_int,c.POINTER(c.c_char_p),c.POINTER(c.c_int)]);compile_=gl('glCompileShader',None,[c.c_uint]);status=gl('glGetShaderiv',None,[c.c_uint,c.c_uint,c.POINTER(c.c_int)]);log=gl('glGetShaderInfoLog',None,[c.c_uint,c.c_int,c.POINTER(c.c_int),c.c_char_p])
js=(Path(__file__).resolve().parents[1]/'module-template/webroot/assets/liquid-glass.js').read_text()
ids=[]
for key,kind in [('vertex',0x8b31),('fragment',0x8b30)]:
 text=re.search(r'const '+key+r' = `([\s\S]*?)`;',js)[1].encode();shader=createshader(kind);p=c.c_char_p(text);source(shader,1,c.byref(p),None);compile_(shader);success=c.c_int();status(shader,0x8b81,c.byref(success))
 if not success.value:
  result=c.create_string_buffer(8192);log(shader,8192,None,result);raise RuntimeError(result.value.decode())
 print('PASS',key,'GLSL compile');ids.append(shader)
create=gl('glCreateProgram',c.c_uint,[]);attach=gl('glAttachShader',None,[c.c_uint,c.c_uint]);link=gl('glLinkProgram',None,[c.c_uint]);programstatus=gl('glGetProgramiv',None,[c.c_uint,c.c_uint,c.POINTER(c.c_int)])
program=create()
for shader in ids:attach(program,shader)
link(program);success=c.c_int();programstatus(program,0x8b82,c.byref(success));assert success.value
print('PASS GPU program link')
import math
W,H=160,100
use=gl('glUseProgram',None,[c.c_uint]);use(program)
bindbuffer=gl('glBindBuffer',None,[c.c_uint,c.c_uint]);genbuffer=gl('glGenBuffers',None,[c.c_int,c.POINTER(c.c_uint)]);bufferdata=gl('glBufferData',None,[c.c_uint,c.c_ssize_t,c.c_void_p,c.c_uint])
buffer=c.c_uint();genbuffer(1,c.byref(buffer));bindbuffer(0x8892,buffer)
vertices=(c.c_float*12)(-1,-1,1,-1,-1,1,-1,1,1,-1,1,1);bufferdata(0x8892,c.sizeof(vertices),vertices,0x88e4)
attrib=gl('glGetAttribLocation',c.c_int,[c.c_uint,c.c_char_p])(program,b'position')
gl('glEnableVertexAttribArray',None,[c.c_uint])(attrib);gl('glVertexAttribPointer',None,[c.c_uint,c.c_int,c.c_uint,c.c_uint,c.c_int,c.c_void_p])(attrib,2,0x1406,0,0,None)
texture=c.c_uint();gl('glGenTextures',None,[c.c_int,c.POINTER(c.c_uint)])(1,c.byref(texture));gl('glBindTexture',None,[c.c_uint,c.c_uint])(0x0de1,texture)
param=gl('glTexParameteri',None,[c.c_uint,c.c_uint,c.c_int])
for key,value in [(0x2801,0x2601),(0x2800,0x2601),(0x2802,0x812f),(0x2803,0x812f)]:param(0x0de1,key,value)
pixels=[]
for y in range(H):
 for x in range(W):pixels.extend([round(x/(W-1)*255),round(y/(H-1)*255),255 if (x//8+y//8)%2 else 0,255])
data=(c.c_ubyte*len(pixels))(*pixels)
gl('glTexImage2D',None,[c.c_uint,c.c_int,c.c_int,c.c_int,c.c_int,c.c_int,c.c_uint,c.c_uint,c.c_void_p])(0x0de1,0,0x1908,W,H,0,0x1908,0x1401,data)
location=gl('glGetUniformLocation',c.c_int,[c.c_uint,c.c_char_p])
def U(name):return location(program,name.encode())
gl('glUniform1i',None,[c.c_int,c.c_int])(U('content'),0)
u1=gl('glUniform1f',None,[c.c_int,c.c_float]);u2=gl('glUniform2f',None,[c.c_int,c.c_float,c.c_float]);u4=gl('glUniform4f',None,[c.c_int,c.c_float,c.c_float,c.c_float,c.c_float])
u2(U('screen'),W,H);u4(U('rect'),0,0,W,H);u1(U('radius'),22);u4(U('tint'),0,0,0,0);u1(U('gradientTint'),0);u1(U('highlightAlpha'),0)
gl('glViewport',None,[c.c_int,c.c_int,c.c_int,c.c_int])(0,0,W,H)
draw=gl('glDrawArrays',None,[c.c_uint,c.c_int,c.c_int]);read=gl('glReadPixels',None,[c.c_int,c.c_int,c.c_int,c.c_int,c.c_uint,c.c_uint,c.c_void_p])
gl('glClearColor',None,[c.c_float,c.c_float,c.c_float,c.c_float])(1,0,1,1)
gl('glClear',None,[c.c_uint])(0x4000)
def sample(x,y):
 x=max(0,min(W-1,x-.5));y=max(0,min(H-1,y-.5));ix,iy=math.floor(x),math.floor(y);fx,fy=x-ix,y-iy
 def pixel(xx,yy,ch):return pixels[(min(H-1,yy)*W+min(W-1,xx))*4+ch]
 return [(pixel(ix,iy,ch)*(1-fx)+pixel(ix+1,iy,ch)*fx)*(1-fy)+(pixel(ix,iy+1,ch)*(1-fx)+pixel(ix+1,iy+1,ch)*fx)*fy for ch in range(3)]
def native_reference(x,y,refraction,dispersion):
 # Direct evaluation of APK AGSL string 93, before tint and Highlight composition.
 coord=(x+.5,y+.5);p=(coord[0]-W/2,coord[1]-H/2);q=(abs(p[0])-(W/2-22),abs(p[1])-(H/2-22));sd=math.hypot(max(q[0],0),max(q[1],0))-22+min(max(q),0)
 if not refraction or -sd>=10:return sample(*coord)
 d=(1-math.sqrt(1-(1-max(0,-sd)/10)**2))*20
 grad_radius=min(22*1.5,W/2,H/2);cq=(abs(p[0])-(W/2-grad_radius),abs(p[1])-(H/2-grad_radius))
 sign=lambda a:1 if a>0 else -1 if a<0 else 0
 if cq[0]>=0 or cq[1]>=0:
  length=math.hypot(max(cq[0],0),max(cq[1],0));g=(sign(p[0])*max(cq[0],0)/length,sign(p[1])*max(cq[1],0)/length)
 else:g=(sign(p[0]),0) if cq[0]>=cq[1] else (0,sign(p[1]))
 center_length=math.hypot(*p);g=(g[0]+.1*p[0]/center_length,g[1]+.1*p[1]/center_length);length=math.hypot(*g);g=(g[0]/length,g[1]/length)
 refracted=(coord[0]+d*g[0],coord[1]+d*g[1]);intensity=dispersion*p[0]*p[1]/(W/2*H/2);spread=(d*g[0]*intensity,d*g[1]*intensity)
 if not dispersion:return sample(*refracted)
 colors=[sample(refracted[0]+spread[0]*factor,refracted[1]+spread[1]*factor) for factor in [1,2/3,1/3,0,-1/3,-2/3,-1]]
 return [sum(colors[i][0]/div for i,div in [(0,3.5),(1,3.5),(2,3.5),(6,7)]),sum(colors[i][1]/div for i,div in [(1,7),(2,3.5),(3,3.5),(4,3.5)]),sum(colors[i][2]/3 for i in [4,5,6])]
points=[(80,50),(2,50),(80,2),(148,14),(18,15),(22,5),(137,92)]
for enabled,dispersion in [(False,0),(True,0),(True,.05),(True,.35)]:
 u4(U('material'),10 if enabled else 0,20,.1,dispersion);draw(4,0,6);output=(c.c_ubyte*(W*H*4))();read(0,0,W,H,0x1908,0x1401,output)
 worst=0
 for x,y in points:
  expected=native_reference(x,y,enabled,dispersion);offset=((H-1-y)*W+x)*4;actual=list(output[offset:offset+3]);error=max(abs(a-b) for a,b in zip(actual,expected));worst=max(worst,error)
  assert error<3.1,(x,y,actual,expected,error)
 assert output[3]==0,'Rounded-corner exterior must remain transparent'
 full=(c.c_ubyte*(BUFFER_W*BUFFER_H*4))();read(0,0,BUFFER_W,BUFFER_H,0x1908,0x1401,full)
 rows=[bytes(full[y*BUFFER_W*4:(y+1)*BUFFER_W*4]) for y in reversed(range(BUFFER_H))]
 assert rows[0][:4]==bytes([255,0,255,255]),'Unused buffer capacity must remain distinguishable'
 # Canvas drawImage uses top-left coordinates: crop stage.height - viewport.height.
 crop=b''.join(row[:W*4] for row in rows[BUFFER_H-H:])
 expected_crop=b''.join(bytes(output[y*W*4:(y+1)*W*4]) for y in reversed(range(H)))
 assert crop==expected_crop,'Shared buffer crop must preserve every viewport pixel'
 print(f'PASS GPU/native AGSL parity refraction={enabled} dispersion={dispersion}, max channel error={worst:.3f}/255')
print('PASS oversized shared draw buffer: bottom-left viewport maps to the correct Canvas crop')
