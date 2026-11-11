import html, sys
W=60
def seg(parts):
    return ''.join(f'<span class="{c}">{html.escape(t)}</span>' if c else html.escape(t) for t,c in parts)
def row(parts, width=W, border=('│','│'), sel=False):
    inner=width-4
    text=''.join(t for t,_ in parts)
    n=len(text)
    if n>inner:
        # cut at end with ellipsis
        out=[];left=inner-1
        for t,c in parts:
            if left<=0: break
            out.append((t[:left],c)); left-=len(t[:left])
        parts=out+[('…','')]
        n=inner
    parts=parts+[(' '*(inner-n),'')]
    body=seg(parts)
    if sel: body=f'<span class="sel">{body}</span>'
    return f'{border[0]} {body} {border[1]}'
def top(title, tabs, width=W, hot='²'):
    t=f'╭─{hot}{title}'
    tabstr=''.join(f'┤ {x} ├' for x in tabs)
    fill=width-len(t)-len(tabstr)-2
    return f'╭─<span class="b">{hot}{title}</span>'+'─'*fill+html.escape(tabstr)+'─╮'
def bot(width=W, tab=None):
    if tab:
        s=f'┤ {tab} ├'; return '╰'+'─'*(width-len(s)-3)+s+'─╯'
    return '╰'+'─'*(width-2)+'╯'
def hdr(cols):
    return [(c,'b') for c in cols]
d='d';ok='ok';inf='info';w='warn';bad='bad';b='b'
def strip(s):
    m={'●':ok,'○':inf,'◌':d,'▲':w,'✕':bad,'–':d}
    out=[]
    for ch in s:
        out.append((ch,m.get(ch,'b')))
    return out
def pad(s,n): return s+' '*(n-len(s))
def cut(s,n): return pad(s if len(s)<=n else s[:n-1]+'…',n)
def setrow(state_strip, sid, summ, age, sel=False):
    st=strip(state_strip)
    slen=len(state_strip)
    return row([('set         ','')]+st+[(' '*(11-slen),''),(cut(sid+' '+summ,27)+' ',''),(age.rjust(5),'')],sel=sel)
lines=[]
lines.append(top('pipeline',['reported > review > integrated']))
lines.append(row([('reported 6  ──►  review 5  ──►  integrated 9','')]))
lines.append(row([('reports      ',''),('████████████',ok),('▒▒',bad),('   6  accepted 5  rejected 1','')]))
lines.append(row([('integrations ',''),('███',ok),('█████',inf),('▒▒',w),('▒',bad),('▓',d),('  9  built 4  in main 1  …','')]))
lines.append(row([('STAGE','b'),('       ',''),('STATE','b'),('      ',''),('WHO / COMMIT','b'),('              ',''),('  AGE','b')]))
lines.append(setrow('◌◌◌◌','1c2d3e4f','building 0/4 built','40s'))
lines.append(setrow('○○◌◌','2a7f0b19','building 2/4 built','1m10s'))
lines.append(setrow('○▲◌◌','3b81e2c4','conflicted: web','2m05s'))
lines.append(setrow('○○✕○','4c90a7d2','failed: worker','3m00s'))
lines.append(setrow('●○○–','8a46cad2','1/3 in main','4m00s',sel=True))
lines.append(row([('report      ',''),('accepted  ',ok),(' ',''),(pad('developer-4 3 repos 9f3c2a1',28),''),('   5m','')]))
lines.append(setrow('○○○–','4b7e91c0','built 3/3','6m20s'))
lines.append(setrow('––––','5d13f6a8','discarded','9m00s'))
lines.append(setrow('○○○○','6e24a8b3','built 4/4','11m'))
lines.append(setrow('●●●●','7f35b9c1','in main 4/4','14m'))
lines.append(setrow('●●●●●●●+25','9a01c3d7','5/32 in main','18m'))
lines.append(row([('── selected ','d'),('─'*44,'d')]))
lines.append(row([('set 8a46cad2 ','b'),('integration/plan-185  1 of 3 in main','')]))
lines.append(row([(' # ','b'),('REPO        ','b'),('STATE       ','b'),('HEAD     ','b'),('VERIFY   ','b'),('    ','')]))
lines.append(row([(' 1 ',''),('api         ',''),('● in main   ',ok),('9f3c2a1  ',''),('passed   ',ok)]))
lines.append(row([('▌2 ','b'),('web         ',''),('○ built     ',inf),('51ab07e  ',''),('passed   ',ok),('next','b')]))
lines.append(row([(' 3 ',''),('worker      ',''),('○ built     ',inf),('c04d9e2  ',''),('running  ',d),('review','')]))
lines.append(row([(' 4 ',''),('infra       ',d),('– dropped   ',d),('–        ',d),('outside the gate',d)]))
lines.append(row([('gate waiting: worker',w)]))
lines.append(bot(tab='5/14'))
open(sys.argv[1]+'/pipe120.html','w').write('\n'.join(lines))

# 80-col narrow: one-column layout, panel 80 wide
W2=80
def row2(parts,sel=False): return row(parts,width=W2,sel=sel)
l2=[top('pipeline',['reported > review > integrated'],width=W2)]
l2.append(row2([('STAGE','b'),('       ',''),('STATE','b'),('      ',''),('WHO / COMMIT','b'),(' '*34,''),('  AGE','b')]))
def setrow2(s,sid,summ,age,sel=False):
    return row2([('set         ','')]+strip(s)+[(' '*(11-len(s)),''),(cut(sid+' '+summ,47)+' ',''),(age.rjust(5),'')],sel=sel)
l2.append(setrow2('●●●●●●●+25','9a01c3d7','5 of 32 in main · next: billing','18m',sel=True))
l2.append(row2([('── selected ','d'),('─'*64,'d')]))
l2.append(row2([('set 9a01c3d7 ','b'),('integration/plan-190  5 of 32 in main · ','') ,('▲ 3 above','d')]))
l2.append(row2([(' # ','b'),('REPO          ','b'),('STATE       ','b'),('HEAD     ','b'),('VERIFY   ','b')]))
l2.append(row2([(' 5 ',''),('ledger-core   ',''),('● in main   ',ok),('3a1b9c0  ',''),('passed',ok)]))
l2.append(row2([('▌6 ','b'),('payme…service ',''),('○ built     ',inf),('51ab07e  ',''),('passed   ',ok),('next','b')]))
l2.append(row2([(' 7 ',''),('notif…worker  ',''),('○ built     ',inf),('77c2d10  ',''),('passed',ok)]))
l2.append(row2([(' 8 ',''),('search-index  ',''),('◌ building  ',d),('–        ',d),('not run',d)]))
l2.append(row2([('▼ 24 below · ',d),('payme…service',''),(' = payments-reconciliation-service','d')]))
l2.append(bot(width=W2,tab='1/1'))
open(sys.argv[1]+'/pipe80.html','w').write('\n'.join(l2))

# agents panel 60 wide with REPOS column and selected worktrees
l3=[top('agents',['5 active','1 ended'],hot='¹')]
l3.append(row([('  ',''),('AGENT','b'),('        ',''),('ROLE','b'),('       ',''),('GEN','b'),(' ',''),('STATE','b'),('    ',''),('REPOS','b'),('   ',''),('AGE','b')]))
def ar(g,a,r,gen,st,rp,age,sel=False): return row([(g+' ',''),(pad(a,13)+pad(r,11)+pad(gen,4)+pad(st,9)+pad(rp,5)+age.rjust(6),'')],sel=sel)
l3.append(ar('*','developer-4','Developer','g1','working','3','3s',True))
l3.append(ar('*','developer-6','Developer','g2','working','2','41s'))
l3.append(ar('●','pm-1','PM','g1','idle','–','1m35s'))
l3.append(row([('── selected ','d'),('─'*44,'d')]))
l3.append(row([('developer-4 ','b'),('worktrees on feat/PM-512-refund-sync','')]))
l3.append(row([('api     ',''),('~/.capstan/wt/developer-4/api',d)]))
l3.append(row([('web     ',''),('~/.capstan/wt/developer-4/web',d)]))
l3.append(row([('worker  ',''),('~/.capstan/wt/developer-4/worker',d)]))
l3.append(row([('change folder  ',''),('~/.capstan/wt/developer-4',d)]))
l3.append(bot())
open(sys.argv[1]+'/agents.html','w').write('\n'.join(l3))
