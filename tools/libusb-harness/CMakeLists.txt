cmake_minimum_required(VERSION 3.20)
project(a16_usb_backend C)
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
option(HARNESS_SANITIZERS "Enable address/undefined behavior sanitizers" OFF)
if(HARNESS_SANITIZERS)
  add_compile_options(-fsanitize=address,undefined -fno-omit-frame-pointer)
  add_link_options(-fsanitize=address,undefined)
endif()
find_package(PkgConfig REQUIRED)
pkg_check_modules(LIBUSB REQUIRED IMPORTED_TARGET libusb-1.0)
pkg_check_modules(PIPEWIRE REQUIRED IMPORTED_TARGET libpipewire-0.3)
pkg_check_modules(SAMPLERATE REQUIRED IMPORTED_TARGET samplerate)
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
  ${OZZY}/common/usb_midi.c)
target_compile_definitions(ozzy-engine PUBLIC OZZY_USER_ENGINE)
target_include_directories(ozzy-engine PUBLIC ${OZZY})
target_link_libraries(ozzy-engine PUBLIC m)
add_executable(a16-usb main.c usb_bus.c wav.c)
target_compile_options(a16-usb PRIVATE -Wall -Wextra -Werror)
target_link_libraries(a16-usb PRIVATE ozzy-engine PkgConfig::LIBUSB)
add_executable(a16-driver pipewire.c usb_bus.c)
target_compile_options(a16-driver PRIVATE -Wall -Wextra -Werror)
target_link_libraries(a16-driver PRIVATE ozzy-engine PkgConfig::LIBUSB PkgConfig::PIPEWIRE PkgConfig::SAMPLERATE)
install(TARGETS a16-usb a16-driver RUNTIME DESTINATION bin)
include(CTest)
if(BUILD_TESTING)
  find_package(Threads REQUIRED)
  find_package(Python3 REQUIRED COMPONENTS Interpreter)
  add_test(NAME upmix-config COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/../pipewire-upmix/generate.py --check)
  add_test(NAME profile-catalog COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/layouts/generate.py --check)
  add_executable(test-live test_live.c)
  target_link_libraries(test-live PRIVATE m Threads::Threads PkgConfig::PIPEWIRE)
  target_compile_options(test-live PRIVATE -UNDEBUG)
  add_test(NAME live-audio COMMAND test-live)
  add_test(NAME pipewire-help COMMAND a16-driver --help)
  add_test(NAME list-profiles COMMAND a16-driver --list-profiles)
  add_test(NAME config-paths COMMAND ${Python3_EXECUTABLE}
    ${CMAKE_CURRENT_SOURCE_DIR}/test_config.py $<TARGET_FILE:a16-driver>)
  add_test(NAME reject-profile COMMAND a16-driver --profile invalid)
  set_tests_properties(reject-profile PROPERTIES WILL_FAIL TRUE)
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
  add_test(NAME help COMMAND a16-usb --help)
  add_test(NAME reject-unbounded COMMAND a16-usb stream)
  set_tests_properties(reject-unbounded PROPERTIES WILL_FAIL TRUE)
  set_tests_properties(a16-offline usb-lifecycle help reject-unbounded PROPERTIES
    ENVIRONMENT "UBSAN_OPTIONS=halt_on_error=1;ASAN_OPTIONS=detect_leaks=1")
endif()

install(PROGRAMS ../pipewire-upmix/a16-upmix DESTINATION bin)
install(FILES ../pipewire-upmix/a16-upmix.conf DESTINATION share/a16-userspace-driver)

add_executable(a16-static-dsp ../pipewire-upmix/static-upmix.c)
target_compile_options(a16-static-dsp PRIVATE -Wall -Wextra -Werror)
target_link_libraries(a16-static-dsp PRIVATE PkgConfig::PIPEWIRE m)
install(TARGETS a16-static-dsp RUNTIME DESTINATION bin)

if(BUILD_TESTING)
  add_test(NAME static-upmix COMMAND ${Python3_EXECUTABLE}
    ${CMAKE_CURRENT_SOURCE_DIR}/../pipewire-upmix/test-static.py
    $<TARGET_FILE:a16-static-dsp> ${CMAKE_CURRENT_SOURCE_DIR}/../pipewire-upmix/a16-upmix.conf)
endif()

install(PROGRAMS ../pipewire-upmix/a16-play-static DESTINATION bin)
