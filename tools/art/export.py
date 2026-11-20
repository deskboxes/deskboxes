"""Turn the rendered stills (tools/art/out/NAME_THEME.png, from render.mjs) into the Settings pictures
in res/settings: NAME.jpg (dark theme) and NAME_l.jpg (light theme) at 800x450, the Home tiles'
t_NAME[_l].jpg at 440x248, and the Home banner home[_l].jpg at 1440x400. The clips (NAME[_l].mp4)
are written by render.mjs itself; their first frame is the still."""
import os
from PIL import Image

A = os.path.dirname(os.path.abspath(__file__))
O = os.path.normpath(os.path.join(A, '..', '..', 'res', 'settings'))
TILES = {'tabs', 'rollup', 'rules', 'az', 'blend', 'colors', 'folder', 'above', 'hide', 'saved', 'search', 'locks', 'privacy'}


def save(im, size, name):
    path = os.path.join(O, name)
    if os.path.exists(path):
        os.remove(path)  # some carry a mandatory label that refuses an overwrite
    im.resize(size, Image.LANCZOS).save(path, 'JPEG', quality=88, optimize=True, progressive=True, subsampling=0)


n = 0
for f in sorted(os.listdir(os.path.join(A, 'out'))):
    if not f.endswith('.png'):
        continue
    name, theme = f[:-4].rsplit('_', 1)
    suf = '_l' if theme == 'light' else ''
    im = Image.open(os.path.join(A, 'out', f)).convert('RGB')
    if name == 'home':
        save(im, (1440, 400), 'home' + suf + '.jpg')
    else:
        save(im, (800, 450), name + suf + '.jpg')
        if name in TILES:
            save(im, (440, 248), 't_' + name + suf + '.jpg')
            n += 1
    n += 1
print(n, 'pictures in', O)
