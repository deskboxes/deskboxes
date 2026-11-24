"""Turn the rendered stills (tools/art/out/NAME_THEME.png, from render.mjs) into the Settings pictures
in res/settings: NAME.jpg (dark theme) and NAME_l.jpg (light theme) at 1280x720, the Home tiles'
t_NAME[_l].jpg at 440x248, and the Home banner home[_l].jpg at 1440x400. The clips (NAME[_l].mp4)
are written by render.mjs itself. A picture with a clip is then that clip's first frame, decoded, so
the swap from picture to video shows no change (the stills were crisper than the video's frames).
python tools/art/export.py [--clips-only]   (--clips-only: just that last step)"""
import os
import subprocess
import sys
from io import BytesIO
from PIL import Image

A = os.path.dirname(os.path.abspath(__file__))
O = os.path.normpath(os.path.join(A, '..', '..', 'res', 'settings'))
TILES = {'tabs', 'rollup', 'rules', 'az', 'blend', 'colors', 'folder', 'above', 'hide', 'saved', 'search', 'locks', 'privacy', 'pages'}


def save(im, size, name):
    path = os.path.join(O, name)
    if os.path.exists(path):
        os.remove(path)  # some carry a mandatory label that refuses an overwrite
    im.resize(size, Image.LANCZOS).save(path, 'JPEG', quality=88, optimize=True, progressive=True, subsampling=0)


n = 0
for f in ([] if '--clips-only' in sys.argv else sorted(os.listdir(os.path.join(A, 'out')))):
    if not f.endswith('.png'):
        continue
    name, theme = f[:-4].rsplit('_', 1)
    suf = '_l' if theme == 'light' else ''
    im = Image.open(os.path.join(A, 'out', f)).convert('RGB')
    if name == 'home':
        save(im, (1440, 400), 'home' + suf + '.jpg')
    else:
        save(im, (1280, 720), name + suf + '.jpg')
        if name in TILES:
            save(im, (440, 248), 't_' + name + suf + '.jpg')
            n += 1
    n += 1

# The pictures with a clip (NAME.mp4 -> NAME.jpg, t_NAME.mp4 -> t_NAME.jpg): its first frame, as a
# player shows it (no resizing: the clip has the picture's size).
c = 0
for f in sorted(os.listdir(O)):
    if not f.endswith('.mp4'):
        continue
    png = subprocess.run(['ffmpeg', '-loglevel', 'error', '-i', os.path.join(O, f), '-frames:v', '1', '-f', 'image2pipe', '-vcodec', 'png', '-'],
                         capture_output=True, check=True).stdout
    im = Image.open(BytesIO(png)).convert('RGB')
    path = os.path.join(O, f[:-4] + '.jpg')
    if os.path.exists(path):
        os.remove(path)
    im.save(path, 'JPEG', quality=95, optimize=True, progressive=True, subsampling=0)
    c += 1
print(n, 'pictures and', c, 'first frames of clips in', O)
