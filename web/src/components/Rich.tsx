import { Fragment, type ReactNode } from 'react'

/** 번역 문구 속 `{이름}` 자리에 링크 같은 요소를 끼워 넣는다(문장 순서는 언어마다 다르므로). */
export function Rich({ text, parts }: { text: string; parts: Record<string, ReactNode> }) {
  return (
    <>
      {text.split(/(\{\w+\})/).map((piece, i) => {
        const name = /^\{(\w+)\}$/.exec(piece)?.[1]
        return <Fragment key={i}>{name && name in parts ? parts[name] : piece}</Fragment>
      })}
    </>
  )
}
