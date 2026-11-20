"""Turn the rendered scenes (tools/art/out/*.png, from render.ps1) into the Settings pictures in
res/settings: NAME.jpg (dark theme) and NAME_l.jpg (light theme) at 800x450, the Home tiles'
close-ups t_NAME[_l].jpg at 440x248, and the Home banner home[_l].jpg at 1440x400."""
import os
from PIL import Image

A = os.path.dirname(os.path.abspath(__file__))
O = os.path.normpath(os.path.join(A, '..', '..', 'res', 'settings'))

n = 0
for f in sorted(os.listdir(os.path.join(A, 'out'))):
    if not f.endswith('.png'):
        continue
    name, theme, thumb = (f[:-4].split('_') + [''])[:3]
    size = (440, 248) if thumb else (1440, 400) if name == 'home' else (800, 450)
    out = ('t_' if thumb else '') + name + ('_l' if theme == 'light' else '') + '.jpg'
    im = Image.open(os.path.join(A, 'out', f)).convert('RGB').resize(size, Image.LANCZOS)
    im.save(os.path.join(O, out), 'JPEG', quality=88, optimize=True, progressive=True, subsampling=0)
    n += 1
print(n, 'pictures in', O)
