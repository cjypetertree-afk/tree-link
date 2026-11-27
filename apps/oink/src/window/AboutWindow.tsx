import { useEffect, useRef, useState } from 'react';
import { Button, Icon, Tag } from 'oink-ui';
import { AppIcon } from './DesktopWindow';
import type { Platform, StageSize } from './DesktopWindow';
import { shell } from '../api/client';
import { inTauri } from '../api/transport';
import { tr } from '../i18n';
const REPO = 'https://github.com/cjypetertree-afk/tree-link';
interface LicenseComponent {
    id: string;
    name: string;
    version: string;
    build?: string | null;
    ecosystem: string;
    repository: string | null;
    licenseExpression: string;
    selectedLicense: string;
    copyright: string[];
    licenseFiles: {
        path: string;
        sha256: string;
    }[];
    scope: string[];
}
interface Inventory {
    schemaVersion: 1;
    components: LicenseComponent[];
}
const GITHUB = 'M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z';
function Wordmark() {
    return (<svg className="about__word" viewBox="0 0 260 64" role="img" aria-label="TREE LINK">
      <text x="130" y="46" textAnchor="middle" fill="currentColor" fontSize="36" fontWeight="800" letterSpacing="3.2" fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif">TREE LINK</text>
    </svg>);
}
function Doodle() {
    return (<svg className="about__doodle" viewBox="0 0 360 160" aria-hidden>
      <path d="M24 151.5H336" strokeDasharray="1 7"/>
      <path d="M128 96 Q180 56 232 96" fill="none" strokeDasharray="4 6" strokeWidth="2"/>
      <circle className="about__dot" cx="180" cy="76" r="3.5"/>
      <g transform="translate(54 58) scale(0.95)">
        <path className="about__fill" d="M43 60 C43 70 40 77 37 82 H63 C60 77 57 70 57 60 Z"/>
        <circle className="about__fill" cx="34" cy="48" r="15"/>
        <circle className="about__fill" cx="66" cy="48" r="15"/>
        <circle className="about__fill" cx="50" cy="36" r="19"/>
        <circle className="about__fill" cx="50" cy="49" r="19"/>
        <path d="M50 17 Q52 9 59 7 Q57 15 50 17 Z"/>
        <circle className="about__dot" cx="42" cy="49" r="2.8"/>
        <circle className="about__dot" cx="58" cy="49" r="2.8"/>
        <path d="M46 53 Q50 56.5 54 53"/>
      </g>
      <g transform="translate(212 64) scale(0.88)">
        <path className="about__fill" d="M43 60 C43 70 40 77 37 82 H63 C60 77 57 70 57 60 Z"/>
        <circle className="about__fill" cx="34" cy="48" r="15"/>
        <circle className="about__fill" cx="66" cy="48" r="15"/>
        <circle className="about__fill" cx="50" cy="36" r="19"/>
        <circle className="about__fill" cx="50" cy="49" r="19"/>
        <path d="M50 17 Q48 9 41 7 Q43 15 50 17 Z"/>
        <circle className="about__dot" cx="42" cy="49" r="2.8"/>
        <circle className="about__dot" cx="58" cy="49" r="2.8"/>
        <path d="M46 53 Q50 56.5 54 53"/>
      </g>
    </svg>);
}
export function AboutWindow({ platform, open, front, version, build, stage, onFront, onClose }: {
    platform: Platform;
    open: boolean;
    front: boolean;
    version: string;
    build?: string | null;
    stage: StageSize;
    onFront: () => void;
    onClose: () => void;
}) {
  const close = (
    platform === 'mac' ? (
      <div className="mac-lights">
        <button type="button" className="mac-light" data-kind="close" aria-label={tr('关闭')} onClick={onClose}>
          <svg viewBox="0 0 12 12" aria-hidden><path d="M3.5 3.5l5 5M8.5 3.5l-5 5" /></svg>
        </button>
        <button type="button" className="mac-light" data-kind="min" aria-label={tr('最小化')} disabled><svg viewBox="0 0 12 12" aria-hidden /></button>
        <button type="button" className="mac-light" data-kind="zoom" aria-label={tr('全屏')} disabled><svg viewBox="0 0 12 12" aria-hidden /></button>
      </div>
    ) : (
      <div className="win-caps">
        <button type="button" className="win-cap" data-kind="close" aria-label={tr('关闭')} onClick={onClose}>
          <svg viewBox="0 0 10 10" aria-hidden><path d="M.5.5l9 9M9.5.5l-9 9" /></svg>
        </button>
      </div>
    )
  );

    const [page, setPage] = useState<'main' | 'licenses'>('main');
    const [inventory, setInventory] = useState<Inventory | null>(null);
    const [selected, setSelected] = useState<string | null>(null);
    const [terms, setTerms] = useState<string>('');
    const [error, setError] = useState(false);
    const termsGeneration = useRef(0);
    useEffect(() => {
        if (!open)
            return;
        void shell.attribution('third-party-licenses.json').then(text => {
            const parsed = JSON.parse(text) as Inventory;
            if (parsed.schemaVersion !== 1 || !Array.isArray(parsed.components))
                throw new Error('schema');
            setInventory(parsed);
        }).catch(() => setError(true));
    }, [open]);
    const showTerms = async (component: LicenseComponent) => {
        const generation = ++termsGeneration.current;
        setSelected(component.id);
        setTerms('');
        setError(false);
        try {
            const texts = await Promise.all(component.licenseFiles.map(file => shell.attribution(file.path)));
            if (termsGeneration.current === generation) setTerms([...component.copyright, ...texts].join('\n\n'));
        }
        catch {
            if (termsGeneration.current === generation) setError(true);
        }
    };
    void stage;
    return <div className="win about" data-platform={platform} data-active={front ? '' : undefined} data-front="" role="dialog" aria-label={tr('关于 Oink')} style={{ left: 0, top: 0, width: '100%', height: '100%' }} onPointerDownCapture={onFront} onKeyDown={e => { if (e.key === 'Escape')
        onClose(); }}>
    <div className="win__titlebar about__bar" data-tauri-drag-region>
      <span hidden={inTauri && platform === 'win'} style={inTauri && platform === 'mac' ? {visibility:'hidden',pointerEvents:'none'} : undefined}>{close}</span>
      {page === 'licenses' && <span className="about__crumb"><Button variant="ghost" size="sm" iconOnly aria-label={tr('返回')} onClick={() => { termsGeneration.current++; if (selected)
        setSelected(null);
    else
        setPage('main'); }}><Icon name="arrow-left"/></Button><span>{tr('开源软件引用')}</span></span>}
    </div>
    {page === 'main' && <Doodle />}
    {page === 'main' ? <div className="about__main">
      <AppIcon platform={platform} size={76}/><Wordmark /><p className="about__ver">{build==null?tr('版本 {0}', version):tr('版本 {0} ({1})', version, build)}</p>
      <Button className="about__repo" variant="ghost" size="sm" onClick={() => { void shell.openUrl(REPO); }} leading={<svg className="about__gh" viewBox="0 0 16 16" aria-hidden><path d={GITHUB}/></svg>}>github.com/cjypetertree-afk/tree-link</Button>
      <Button className="about__oss" variant="ghost" size="sm" trailing={<Icon name="chevron-right"/>} onClick={() => setPage('licenses')}>{tr('开源软件引用')}</Button>
      <Button variant="ghost" size="sm" onClick={() => { const generation=++termsGeneration.current; setPage('licenses'); setSelected('product'); setTerms(''); setError(false); void Promise.all(['LICENSE', 'LICENSE-ADDITIONAL-TERMS.md'].map(p => shell.attribution(p))).then(texts => {if(termsGeneration.current===generation)setTerms(texts.join('\n\n'));}).catch(() => {if(termsGeneration.current===generation)setError(true);}); }}>GPL-3.0 · {tr('附加条款')}</Button>
      <p className="about__copy">© 2026 TREE LINK · GPL-3.0 (derived from Oink)</p>
    </div> : <div className="about__licenses ok-scroll">
      {error && <p role="alert">{tr('无法读取许可文件')}</p>}
      {selected ? <pre className="license-text">{terms || tr('正在读取…')}</pre> : <><p className="about__lead">{tr('Oink 使用了以下开源软件，感谢它们的作者。')}</p>
        {!inventory && <p role="status">{tr('正在读取…')}</p>}
        <ul>{inventory?.components.map(l => <li key={l.id}>
          <button type="button" className="about__license-link ok-focusable" onClick={() => { void showTerms(l); }}><span className="about__lic-name">{l.name}<span className="about__lic-ver">{l.version}</span></span><span className="about__lic-note">{l.copyright.join(' · ') || l.name} · {l.scope.join(', ')}</span></button>
          <Tag className="about__lic-tag" size="sm">{l.licenseExpression}</Tag>
          {l.repository && /^https?:\/\//.test(l.repository) && <Button size="sm" variant="ghost" iconOnly aria-label={tr('项目主页')} onClick={() => { void shell.openUrl(l.repository!); }}><Icon name="external"/></Button>}
        </li>)}</ul></>}
    </div>}
  </div>;
}
