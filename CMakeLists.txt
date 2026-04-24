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

add_executable(
  potato_gsim
  main.cpp
  src/net/MpClient.cpp
  src/net/MpClientSim.cpp
  src/net/MpReplay.cpp
  src/net/Protocol.cpp
  src/render/Renderer.cpp
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
target_include_directories(potato_gsim PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}")
target_link_libraries(potato_gsim PRIVATE SFML::Graphics SFML::Window SFML::System enet)

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
target_include_directories(potato_gsim_server PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}")
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
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  BUNDLE DESTINATION .
)

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
  target_include_directories(parity_regression_tests PRIVATE src)
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
  target_include_directories(high_load_smoke PRIVATE src)
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
  target_include_directories(throughput_benchmark PRIVATE src)
  target_link_libraries(throughput_benchmark PRIVATE SFML::System)

  add_executable(
    algorithm_switching_tests
    tests/algorithm_switching_tests.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(algorithm_switching_tests PRIVATE src)
  target_link_libraries(algorithm_switching_tests PRIVATE SFML::System)

  add_test(NAME parity_regression_tests COMMAND parity_regression_tests)
  add_test(NAME high_load_smoke COMMAND high_load_smoke)
  add_test(NAME algorithm_switching_tests COMMAND algorithm_switching_tests)

  add_executable(
    mp_protocol_v3_tests
    tests/mp_protocol_v3_tests.cpp
    src/net/Protocol.cpp
  )
  target_include_directories(mp_protocol_v3_tests PRIVATE src)
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
  target_include_directories(mp_world_sync_tests PRIVATE src "${POTATO_GSIM_ENET_INCLUDE}")
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
