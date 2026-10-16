/**
 * 홈 데모 영상 칸의 사람 그림. 사진 대신 인라인 SVG라 추가 요청이 없고(각 1KB 남짓), 초상권 걱정도 없다.
 * 칸을 꽉 채우도록 잘라 그린다(slice). 말하는 동안 입(`demo-mouth`)이 CSS로 움직인다.
 */
export type Face = 'sakura' | 'minjun'

export function DemoFace({ who }: { who: Face }) {
  return (
    <svg className="demo-face" viewBox="0 0 320 200" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      {who === 'sakura' ? <Sakura /> : <Minjun />}
    </svg>
  )
}

function Sakura() {
  return (
    <>
      <rect width="320" height="200" fill="#E8DED2" />
      <rect x="236" y="26" width="58" height="74" rx="4" fill="#F4EEE6" />
      <path d="M236 63h58M265 26v74" stroke="#DCD1C3" strokeWidth="3" />
      <rect x="34" y="142" width="34" height="58" rx="6" fill="#C9B8A4" />
      <ellipse cx="42" cy="128" rx="11" ry="22" fill="#7FA07A" transform="rotate(-20 42 128)" />
      <ellipse cx="60" cy="124" rx="10" ry="24" fill="#6B8F67" transform="rotate(18 60 124)" />
      <path d="M117 100c-4-52 90-52 86 0l4 38c-26 10-68 10-94 0z" fill="#2B211E" />
      <rect x="150" y="118" width="20" height="26" rx="8" fill="#EBC3A5" />
      <path d="M84 200c4-38 34-56 76-56s72 18 76 56z" fill="#3A4660" />
      <path d="M145 145l15 22 15-22z" fill="#F4F1EA" />
      <ellipse cx="160" cy="94" rx="30" ry="35" fill="#F2CDB0" />
      <path d="M127 96C121 46 199 46 193 96 186 82 172 78 160 78s-26 4-33 18z" fill="#2B211E" />
      <path d="M142 90q6-3 12 0M166 90q6-3 12 0" stroke="#3A2A24" strokeWidth="2" fill="none" strokeLinecap="round" />
      <ellipse cx="148" cy="99" rx="2.6" ry="3" fill="#2B211E" />
      <ellipse cx="172" cy="99" rx="2.6" ry="3" fill="#2B211E" />
      <ellipse cx="141" cy="110" rx="6" ry="3.5" fill="#E9A39A" opacity=".45" />
      <ellipse cx="179" cy="110" rx="6" ry="3.5" fill="#E9A39A" opacity=".45" />
      <ellipse className="demo-mouth" cx="160" cy="116" rx="5.5" ry="2.2" fill="#B5574C" />
    </>
  )
}

function Minjun() {
  return (
    <>
      <rect width="320" height="200" fill="#D8DEE4" />
      <rect x="22" y="40" width="74" height="160" fill="#C4CCD4" />
      <path d="M22 96h74M22 152h74" stroke="#AEB8C2" strokeWidth="4" />
      <rect x="30" y="64" width="9" height="30" fill="#7D93A8" />
      <rect x="41" y="70" width="8" height="24" fill="#C08A5A" />
      <rect x="51" y="62" width="10" height="32" fill="#5F7566" />
      <rect x="32" y="122" width="10" height="28" fill="#B86A5C" />
      <rect x="44" y="118" width="8" height="32" fill="#8E9DAA" />
      <rect x="150" y="118" width="20" height="26" rx="8" fill="#E2B892" />
      <path d="M84 200c4-38 34-56 76-56s72 18 76 56z" fill="#5B7F72" />
      <path d="M143 146l17 18 17-18z" fill="#EEF0F2" />
      <ellipse cx="129" cy="97" rx="5" ry="8" fill="#E2B892" />
      <ellipse cx="191" cy="97" rx="5" ry="8" fill="#E2B892" />
      <ellipse cx="160" cy="94" rx="30" ry="35" fill="#EAC09E" />
      <path d="M128 90c-6-40 68-46 64-2-6-12-14-18-30-18-16 0-28 6-34 20z" fill="#1F1B1A" />
      <path d="M142 89h12M166 89h12" stroke="#1F1B1A" strokeWidth="2.6" strokeLinecap="round" />
      <rect x="138" y="93" width="19" height="13" rx="5" fill="none" stroke="#2A2A2A" strokeWidth="2" />
      <rect x="163" y="93" width="19" height="13" rx="5" fill="none" stroke="#2A2A2A" strokeWidth="2" />
      <path d="M157 98h6" stroke="#2A2A2A" strokeWidth="2" />
      <ellipse cx="147.5" cy="99.5" rx="2.3" ry="2.6" fill="#1F1B1A" />
      <ellipse cx="172.5" cy="99.5" rx="2.3" ry="2.6" fill="#1F1B1A" />
      <ellipse className="demo-mouth" cx="160" cy="117" rx="6" ry="2.2" fill="#A9584B" />
    </>
  )
}
