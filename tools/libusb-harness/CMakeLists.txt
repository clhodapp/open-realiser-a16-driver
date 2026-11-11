cmake_minimum_required(VERSION 3.20)
project(ozzy_libusb C)
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
option(HARNESS_SANITIZERS "Enable address/undefined behavior sanitizers" OFF)
if(HARNESS_SANITIZERS)
  add_compile_options(-fsanitize=address,undefined -fno-omit-frame-pointer)
  add_link_options(-fsanitize=address,undefined)
endif()
find_package(PkgConfig REQUIRED)
pkg_check_modules(LIBUSB REQUIRED IMPORTED_TARGET libusb-1.0)
set(OZZY ${CMAKE_CURRENT_SOURCE_DIR}/../..)
add_library(ozzy-engine STATIC
  ${OZZY}/engine/core/ozzy_engine.c
  ${OZZY}/engine/core/ozzy_isoc_output.c
  ${OZZY}/engine/core/ozzy_isoc_input.c
  ${OZZY}/engine/devices/ploytec/ploytec_engine.c
  ${OZZY}/engine/devices/ploytec/ploytec_io.c
  ${OZZY}/engine/devices/ploytec/ploytec_isoc_engine.c
  ${OZZY}/engine/devices/ploytec/ploytec_us3xx_engine.c
  ${OZZY}/engine/devices/usbmidi/usb_midi_engine.c
  ${OZZY}/common/devices/ploytec/ploytec_handshake.c
  ${OZZY}/common/devices/ploytec/ploytec_codec.c
  ${OZZY}/common/usb_midi.c
  ${OZZY}/common/midi_ump.c)
target_compile_definitions(ozzy-engine PUBLIC OZZY_USER_ENGINE)
target_include_directories(ozzy-engine PUBLIC ${OZZY})
target_link_libraries(ozzy-engine PUBLIC m)
add_executable(ozzy-libusb main.c usb_bus.c wav.c)
target_compile_options(ozzy-libusb PRIVATE -Wall -Wextra -Werror)
target_link_libraries(ozzy-libusb PRIVATE ozzy-engine PkgConfig::LIBUSB)
install(TARGETS ozzy-libusb RUNTIME DESTINATION bin)
include(CTest)
if(BUILD_TESTING)
  add_executable(test-a16 test_a16.c wav.c ${OZZY}/tests/fake_bus.c)
  target_link_libraries(test-a16 PRIVATE ozzy-engine)
  target_compile_options(test-a16 PRIVATE -UNDEBUG)
  add_test(NAME a16-offline COMMAND test-a16)
  add_executable(test-usb-lifecycle test_usb.c usb_bus.c)
  target_link_libraries(test-usb-lifecycle PRIVATE ozzy-engine PkgConfig::LIBUSB)
  target_compile_options(test-usb-lifecycle PRIVATE -UNDEBUG)
  target_link_options(test-usb-lifecycle PRIVATE
    -Wl,--wrap=libusb_submit_transfer -Wl,--wrap=libusb_cancel_transfer
    -Wl,--wrap=libusb_handle_events_timeout -Wl,--wrap=libusb_control_transfer)
  add_test(NAME usb-lifecycle COMMAND test-usb-lifecycle)
  add_test(NAME help COMMAND ozzy-libusb --help)
  add_test(NAME reject-unbounded COMMAND ozzy-libusb stream)
  set_tests_properties(reject-unbounded PROPERTIES WILL_FAIL TRUE)
  set_tests_properties(a16-offline usb-lifecycle help reject-unbounded PROPERTIES
    ENVIRONMENT "UBSAN_OPTIONS=halt_on_error=1;ASAN_OPTIONS=detect_leaks=1")
endif()
