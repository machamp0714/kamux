import { readFileSync } from 'node:fs';
const css = readFileSync('/Users/ooidetatsuya/repo/kamux/.claude/skills/kamux-design-system/tokens.css','utf8');
const lin=c=>c<=0.03928?c/12.92:Math.pow((c+0.055)/1.055,2.4);
const L=h=>{h=h.replace('#','');const[r,g,b]=[0,2,4].map(i=>lin(parseInt(h.slice(i,i+2),16)/255));return 0.2126*r+0.7152*g+0.0722*b;};
const ratio=(a,b)=>{const l1=L(a),l2=L(b);const[hi,lo]=l1>l2?[l1,l2]:[l2,l1];return(hi+0.05)/(lo+0.05);};

// :root ブロックと data-theme='light' ブロックを実ファイルから読む
const grab = (start) => {
  const i = css.indexOf(start); const body = css.slice(i, css.indexOf('\n}', i));
  const t = {}; for (const m of body.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-f]{6})/g)) t[m[1]] = m[2];
  return t;
};
const T = { dark: grab(':root {'), light: grab(":root[data-theme='light'] {") };

const TEXT=['text-primary','text-secondary','text-muted','accent','state-running','state-waiting',
            'state-idle','state-exited','state-interrupted','state-error'];
const BG=['bg-app','bg-surface','bg-elevated','bg-hover'];
let fail=0;
for(const theme of ['dark','light']){
  const t=T[theme];
  for(const fg of TEXT) for(const bg of BG){
    const r=ratio(t[fg],t[bg]);
    if(r<4.5){fail++;console.log(`✗ ${theme} ${fg} on ${bg} = ${r.toFixed(2)}`);}
  }
  const ac=ratio(t.accent,t['accent-soft']);
  if(ac<4.5){fail++;console.log(`✗ ${theme} accent on accent-soft = ${ac.toFixed(2)}`);}
  for(const bg of ['bg-surface','bg-elevated']){
    const r=ratio(t['border-input'],t[bg]);
    if(r<3.0){fail++;console.log(`✗ ${theme} border-input on ${bg} = ${r.toFixed(2)} (UI 3:1)`);}
  }
  console.log(`${theme}: accent/accent-soft ${ac.toFixed(2)} | border-input/bg-surface ${ratio(t['border-input'],t['bg-surface']).toFixed(2)}`);
}
console.log(fail===0 ? '\n✅ 全 88 組み合わせが基準を満たす' : `\n❌ ${fail} 件が未達`);
process.exit(fail===0?0:1);
