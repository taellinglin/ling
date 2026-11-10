# Hello World

Ling supports any Unicode identifier. All built-in keywords work in every
supported language simultaneously in the same source file.

## English

```ling
bind start = do {
    print("Hello, World!")
}
```

## Chinese 中文

```ling
令 启 = 执 {
    印("你好，世界！")
}
```

## Thai ภาษาไทย

```ling
ผูก เริ่ม = ทำ {
    พิมพ์("สวัสดีชาวโลก")
}
```

## Korean 한국어

```ling
바인드 시작 = 실행 {
    print("안녕하세요, 세계!")
}
```

## Mixed (all valid simultaneously)

```ling
令 启 = 执 {
    若 true {
        印("Hello from 中文!")
    }
    bind x = 42
    println(x)
}
```
