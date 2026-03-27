#!/bin/bash

# LED控制脚本
# 用于控制RGB LED灯的不同效果
# 使用方法: sh light_control.sh [option]
# 选项: white, red, green, blue, blink, off

# RGB白灯（255，255，255）常亮，亮度最大
white_light_on() {
    echo "开启白色常亮LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0xFFFFFF64050000 >/proc/tm/shell
}

# RGB红灯（255，0，0）常亮，亮度最大
red_light_on() {
    echo "开启红色常亮LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0xFF000064050000 >/proc/tm/shell
}
 
# RGB绿灯（0，255，0）常亮，亮度最大
green_light_on() {
    echo "开启绿色常亮LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0xFF0064050000 >/proc/tm/shell
}

# RGB蓝灯（0，0，255）常亮，亮度最大
blue_light_on() {
    echo "开启蓝色常亮LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0xFF64050000 >/proc/tm/shell
}

# RGB白灯（255，255，255）闪烁，速度最快，亮度最低
white_light_blink() {
    echo "开启白色闪烁LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0x2FFFFFF01010000 >/proc/tm/shell
}

# 关闭所有LED
turn_off_all() {
    echo "关闭所有LED..."
    echo -f ledboard_gpio_i2c_write_data 0x38,0x00000000000000 >/proc/tm/shell
}

# 显示帮助信息
show_help() {
    echo "LED控制脚本"
    echo "使用方法: sh light_control.sh [option]"
    echo "选项:"
    echo "  white  - 白色常亮"
    echo "  red    - 红色常亮"
    echo "  green  - 绿色常亮"
    echo "  blue   - 蓝色常亮"
    echo "  blink  - 白色闪烁"
    echo "  off    - 关闭所有LED"
    echo "  help   - 显示帮助信息"
}

# 主程序入口
main() {
    case "$1" in
        white)
            white_light_on
            ;;
        red)
            red_light_on
            ;;
        green)
            green_light_on
            ;;
        blue)
            blue_light_on
            ;;
        blink)
            white_light_blink
            ;;
        off)
            turn_off_all
            ;;
        help|"")
            show_help
            ;;
        *)
            echo "未知选项: $1"
            show_help
            exit 1
            ;;
    esac
}

# 执行主程序
main "$@"
