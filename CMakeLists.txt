cmake_minimum_required(VERSION 3.20)
project(potato_gsim LANGUAGES CXX C)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

find_package(SFML 3 REQUIRED COMPONENTS Graphics Window System)

include(FetchContent)
FetchContent_Declare(
  enet
  GIT_REPOSITORY https://github.com/lsalzman/enet.git
  GIT_TAG        v1.3.18
)
set(BUILD_SHARED_LIBS OFF)
FetchContent_MakeAvailable(enet)

# ENet's CMakeLists uses directory-scoped include_directories; propagate for dependents.
set(POTATO_GSIM_ENET_INCLUDE "${enet_SOURCE_DIR}/include")
set(POTATO_GSIM_THIRD_PARTY_INCLUDE "${CMAKE_SOURCE_DIR}/include")

add_executable(
  potato_gsim
  main.cpp
  src/net/MpClient.cpp
  src/net/MpClientSim.cpp
  src/net/MpReplay.cpp
  src/net/Protocol.cpp
  src/render/Renderer.cpp
  src/scenario/ScenarioManager.cpp
  src/sim/QuadTree.cpp
  src/sim/SimulationEngine.cpp
  src/sim/ThreadPool.cpp
  src/sim/UniformGrid.cpp
  src/ui/InputActions.cpp
  src/ui/InspectorOverlay.cpp
  src/ui/MenuOverlay.cpp
  src/ui/Predictor.cpp
  src/ui/QuantityFormat.cpp
  src/ui/SelectionState.cpp
  src/ui/TraceStore.cpp
)
set_target_properties(potato_gsim PROPERTIES
  MACOSX_BUNDLE TRUE
)
target_include_directories(
  potato_gsim PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}" "${POTATO_GSIM_THIRD_PARTY_INCLUDE}"
)
target_link_libraries(potato_gsim PRIVATE SFML::Graphics SFML::Window SFML::System enet)
if(APPLE)
  set_target_properties(potato_gsim PROPERTIES
    INSTALL_RPATH "@executable_path/../Frameworks"
  )
elseif(UNIX AND NOT APPLE)
  set_target_properties(potato_gsim PROPERTIES
    INSTALL_RPATH "$ORIGIN/../${CMAKE_INSTALL_LIBDIR}"
  )
endif()

add_executable(
  potato_gsim_server
  src/server/main_server.cpp
  src/net/Protocol.cpp
  src/scenario/ScenarioManager.cpp
  src/sim/QuadTree.cpp
  src/sim/SimulationEngine.cpp
  src/sim/ThreadPool.cpp
  src/sim/UniformGrid.cpp
)
target_include_directories(
  potato_gsim_server PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}" "${POTATO_GSIM_THIRD_PARTY_INCLUDE}"
)
target_link_libraries(potato_gsim_server PRIVATE enet)
if(APPLE)
  find_library(CF_LIBRARY CoreFoundation REQUIRED)
  target_link_libraries(potato_gsim_server PRIVATE ${CF_LIBRARY})
endif()
if(UNIX AND NOT APPLE)
  target_link_libraries(potato_gsim_server PRIVATE m)
endif()
if(WIN32)
  target_link_libraries(potato_gsim_server PRIVATE winmm ws2_32)
endif()

if(MSVC)
  target_compile_options(potato_gsim PRIVATE /W4 /permissive-)
else()
  target_compile_options(potato_gsim PRIVATE -Wall -Wextra -Wpedantic)
endif()

include(GNUInstallDirs)
install(TARGETS potato_gsim
  RUNTIME_DEPENDENCY_SET potato_gsim_runtime_deps
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  BUNDLE DESTINATION .
)
install(TARGETS potato_gsim_server
  RUNTIME_DEPENDENCY_SET potato_gsim_runtime_deps
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)
install(
  RUNTIME_DEPENDENCY_SET potato_gsim_runtime_deps
  PRE_EXCLUDE_REGEXES
    "api-ms-win-.*"
    "ext-ms-win-.*"
  POST_EXCLUDE_REGEXES
    ".*/System/Library/.*"
    ".*/usr/lib/.*"
    ".*/Windows/System32/.*"
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
  FRAMEWORK DESTINATION Frameworks
)

if(APPLE)
  install(CODE [[
    include(BundleUtilities)
    set(BU_CHMOD_BUNDLE_ITEMS ON)
    get_filename_component(_potato_abs_install_prefix "${CMAKE_INSTALL_PREFIX}" ABSOLUTE)
    set(_potato_app "${_potato_abs_install_prefix}/potato_gsim.app")
    set(_potato_extra_dirs "${_potato_abs_install_prefix}/Frameworks;${_potato_abs_install_prefix}/lib")
    fixup_bundle("${_potato_app}" "" "${_potato_extra_dirs}")
    verify_app("${_potato_app}")
    execute_process(
      COMMAND codesign --force --deep --sign - "${_potato_app}"
      RESULT_VARIABLE _potato_codesign_result
    )
    if(NOT _potato_codesign_result EQUAL 0)
      message(FATAL_ERROR "codesign failed for ${_potato_app}")
    endif()
  ]])
endif()

include(CTest)
if(BUILD_TESTING)
  add_executable(
    parity_regression_tests
    tests/parity_regression_tests.cpp
    src/io/Persistence.cpp
    src/scenario/ScenarioManager.cpp
    src/ui/CreationTool.cpp
    src/ui/Predictor.cpp
    src/ui/QuantityFormat.cpp
  )
  target_include_directories(parity_regression_tests PRIVATE src "${POTATO_GSIM_THIRD_PARTY_INCLUDE}")
  target_link_libraries(parity_regression_tests PRIVATE SFML::System)

  add_executable(
    high_load_smoke
    tests/high_load_smoke.cpp
    src/scenario/ScenarioManager.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(high_load_smoke PRIVATE src "${POTATO_GSIM_THIRD_PARTY_INCLUDE}")
  target_link_libraries(high_load_smoke PRIVATE SFML::System)

  add_executable(
    throughput_benchmark
    tests/throughput_benchmark.cpp
    src/scenario/ScenarioManager.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(throughput_benchmark PRIVATE src "${POTATO_GSIM_THIRD_PARTY_INCLUDE}")
  target_link_libraries(throughput_benchmark PRIVATE SFML::System)

  add_executable(
    algorithm_switching_tests
    tests/algorithm_switching_tests.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(algorithm_switching_tests PRIVATE src "${POTATO_GSIM_THIRD_PARTY_INCLUDE}")
  target_link_libraries(algorithm_switching_tests PRIVATE SFML::System)

  add_test(NAME parity_regression_tests COMMAND parity_regression_tests)
  add_test(NAME high_load_smoke COMMAND high_load_smoke)
  add_test(NAME algorithm_switching_tests COMMAND algorithm_switching_tests)

  add_executable(
    mp_protocol_v3_tests
    tests/mp_protocol_v3_tests.cpp
    src/net/Protocol.cpp
  )
  target_include_directories(mp_protocol_v3_tests PRIVATE src "${POTATO_GSIM_THIRD_PARTY_INCLUDE}")
  add_test(NAME mp_protocol_v3_tests COMMAND mp_protocol_v3_tests)

  add_executable(
    mp_world_sync_tests
    tests/mp_world_sync_tests.cpp
    src/net/MpClient.cpp
    src/net/MpClientSim.cpp
    src/net/Protocol.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(
    mp_world_sync_tests PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}" "${POTATO_GSIM_THIRD_PARTY_INCLUDE}"
  )
  target_compile_definitions(mp_world_sync_tests PRIVATE POTATO_GSIM_MP_WORLD_SYNC_TESTS=1)
  target_link_libraries(mp_world_sync_tests PRIVATE SFML::System enet)
  if(UNIX AND NOT APPLE)
    target_link_libraries(mp_world_sync_tests PRIVATE m)
  endif()
  if(WIN32)
    target_link_libraries(mp_world_sync_tests PRIVATE winmm ws2_32)
  endif()
  add_test(NAME mp_world_sync_tests COMMAND mp_world_sync_tests)
endif()
