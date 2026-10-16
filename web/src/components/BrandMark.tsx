/** tolkmeet 마크 "Pair": 원문(위)과 번역(아래)이 나란히 놓인 두 줄. 색은 디자인 토큰 --brand-ink / --brand-accent. */
export function BrandMark({ size = 32 }: { size?: number }) {
  return (
    <svg className="brand-mark" width={size} height={size} viewBox="0 0 96 96" aria-hidden="true" focusable="false">
      <rect x="8" y="28" width="62" height="16" rx="8" fill="var(--brand-ink)" />
      <rect x="26" y="52" width="62" height="16" rx="8" fill="var(--brand-accent)" />
    </svg>
  )
}
